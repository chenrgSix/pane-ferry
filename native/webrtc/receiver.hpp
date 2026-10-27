#pragma once
#include "wire.hpp"
#include <rtc/rtc.hpp>
#include <atomic>
#include <chrono>
#include <functional>
#include <map>
#include <mutex>
#include <optional>

namespace remoteapp {
struct Budget {
    std::atomic<size_t> used{0};
    bool acquire(size_t bytes) {
        auto old=used.load();
        do { if (bytes>32*1024*1024-old) return false; } while (!used.compare_exchange_weak(old,old+bytes));
        return true;
    }
    void release(size_t bytes) { used.fetch_sub(bytes); }
};
/* No playout delay for complete frames. Only a packet gap enters the repair
 * window: RTCP NACK at most every 10ms, then PLI after 60ms. Incomplete frames
 * NEVER enter VideoToolbox. Both packet storage and frame lookahead are bounded. */
class Receiver final : public rtc::MediaHandler {
    using Clock=std::chrono::steady_clock;
    struct Assembly {
        std::map<int64_t,rtc::message_ptr> packets;
        std::optional<int64_t> marker,start;
        Clock::time_point created=Clock::now(), nack=created-std::chrono::seconds(1);
        size_t bytes=0;
    };
    std::mutex mutex;
    std::map<int64_t,Assembly> frames;
    std::optional<int64_t> sequenceRef,timestampRef,expected,lastFrame;
    std::shared_ptr<Budget> budget;
    rtc::message_callback send;
    rtc::RtcpReceivingSession reports;
    rtc::H264RtpDepacketizer depacketizer{rtc::NalUnit::Separator::LongStartSequence};
    std::function<void(Frame)> deliver;
    std::function<void()> loss;
    uint32_t ssrc=0;
    bool awaitingKeyframe=true;
    Clock::time_point lastPLI=Clock::now()-std::chrono::seconds(1);
    void clear() {
        for (auto &[_,frame]:frames) budget->release(frame.bytes);
        frames.clear();expected.reset();
    }
    void pli() {
        awaitingKeyframe=true;
        if (send && Clock::now()-lastPLI>=std::chrono::milliseconds(40)) {
            lastPLI=Clock::now(); reports.requestKeyframe(send);
        }
        loss();
    }
    void nack(const std::vector<uint16_t> &missing) {
        if (!send || missing.empty()) return;
        Bytes bytes{0x81,205};
        const size_t words=2+missing.size();
        bytes.push_back(uint8_t(words>>8));bytes.push_back(uint8_t(words));
        put32(bytes,0x52415050);put32(bytes,ssrc);
        for (uint16_t seq:missing) {bytes.push_back(uint8_t(seq>>8));bytes.push_back(uint8_t(seq));bytes.push_back(0);bytes.push_back(0);}
        auto packet=rtc::make_message(bytes.size(),rtc::Message::Control);
        std::memcpy(packet->data(),bytes.data(),bytes.size());send(packet);
    }
    void flush() {
        while (!frames.empty()) {
            auto current=frames.begin();auto &frame=current->second;
            if (frame.packets.empty()) {frames.erase(current);continue;}
            int64_t begin=expected.value_or(frame.start.value_or(frame.packets.begin()->first));
            auto end=frame.marker;
            auto next=std::next(current);
            if (!end && next!=frames.end() && !next->second.packets.empty()) end=next->second.packets.begin()->first-1;
            if (end && (*end<begin || *end-begin>16384)) {clear();pli();return;}
            std::vector<uint16_t> missing;
            if (end) {
                for (int64_t i=begin;i<=*end && missing.size()<128;++i)
                    if (!frame.packets.count(i)) missing.push_back(uint16_t(i));
            }
            if (end && frame.marker && frame.start && missing.empty()) {
                rtc::message_vector packets;
                for (auto &[seq,packet]:frame.packets) if (seq>=begin && seq<=*end) packets.push_back(std::move(packet));
                expected=*end+1;lastFrame=current->first;
                budget->release(frame.bytes);frames.erase(current);
                depacketizer.incomingChain(packets,send);
                for (const auto &packet:packets) {
                    auto decoded=unpack(reinterpret_cast<const uint8_t *>(packet->data()),packet->size());
                    if (awaitingKeyframe && !decoded.keyframe) {pli();continue;}
                    if (decoded.keyframe) awaitingKeyframe=false;
                    deliver(std::move(decoded));
                }
                continue;
            }
            const auto now=Clock::now();
            if (now-frame.created>=std::chrono::milliseconds(60)) {clear();pli();return;}
            if (!missing.empty() && now-frame.created>=std::chrono::milliseconds(2) && now-frame.nack>=std::chrono::milliseconds(10)) {
                nack(missing);frame.nack=now;
            }
            return;
        }
    }
public:
    Receiver(std::shared_ptr<Budget> b,std::function<void(Frame)> ready,std::function<void()> lost)
        : budget(std::move(b)),deliver(std::move(ready)),loss(std::move(lost)) {}
    ~Receiver() override {clear();}
    void incoming(rtc::message_vector &messages,const rtc::message_callback &callback) override {
        std::lock_guard lock(mutex);send=callback;
        try {
            reports.incoming(messages,callback);
            for (auto &message:messages) {
                if (message->type==rtc::Message::Control) continue;
                auto header=RTP::parse(reinterpret_cast<const uint8_t *>(message->data()),message->size());
                auto seq=sequenceRef?extend16(header.sequence,*sequenceRef):int64_t(header.sequence);
                auto stamp=timestampRef?extend32(header.timestamp,*timestampRef):int64_t(header.timestamp);
                sequenceRef=std::max(sequenceRef.value_or(seq),seq);timestampRef=std::max(timestampRef.value_or(stamp),stamp);
                if (lastFrame && stamp<=*lastFrame) continue;
                ssrc=header.ssrc;
                if (!frames.count(stamp) && frames.size()>=8) {clear();pli();}
                auto &frame=frames[stamp];
                if (frame.packets.count(seq)) continue;
                if (frame.packets.size()>=16384 || frame.bytes+message->size()>MaxFrame || !budget->acquire(message->size())) {
                    clear();pli();continue;
                }
                const auto *data=reinterpret_cast<const uint8_t *>(message->data());
                const auto offset=header.payloadOffset;
                const auto type=data[offset]&31;
                bool seiStart=type==6 || (type==28 && offset+1<message->size() && (data[offset+1]&0x9f)==0x86);
                if (seiStart && std::search(data+offset,data+std::min(message->size(),offset+128),UUID.begin(),UUID.end())!=data+std::min(message->size(),offset+128)) frame.start=seq;
                frame.bytes+=message->size();frame.packets[seq]=std::move(message);
                if (header.marker) frame.marker=seq;
            }
            messages.clear();flush();
        } catch (...) {messages.clear();clear();pli();}
    }
    void outgoing(rtc::message_vector &messages,const rtc::message_callback &) override {
        for (auto &message:messages) message->type=rtc::Message::Control;
    }
    bool requestKeyframe(const rtc::message_callback &callback) override {
        std::lock_guard lock(mutex);send=callback;clear();pli();return true;
    }
    void tick() {
        std::lock_guard lock(mutex);
        try {flush();} catch (...) {clear();pli();}
    }
};
} // namespace remoteapp
