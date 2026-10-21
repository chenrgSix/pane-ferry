#include "peer.h"
#include "receiver.hpp"
#include <rtc/rtc.hpp>
#include <atomic>
#include <chrono>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <unordered_map>

namespace {
using namespace remoteapp;
struct Track {
    std::shared_ptr<rtc::Track> channel;
    std::shared_ptr<Receiver> receiver;
    uint32_t timestamp=0;
};
struct Peer : std::enable_shared_from_this<Peer> {
    bool sending;
    ra_rtc_callback callback;
    void *context;
    std::mutex callbackMutex,tracksMutex;
    std::shared_ptr<rtc::PeerConnection> pc;
    std::unordered_map<uint32_t,Track> tracks;
    std::shared_ptr<Budget> budget=std::make_shared<Budget>();
    std::atomic<bool> stopped{false};
    std::thread timer;
    Peer(bool sender,ra_rtc_callback cb,void *ctx):sending(sender),callback(cb),context(ctx) {}
    void emit(int kind,const std::string &label,const uint8_t *bytes=nullptr,size_t size=0) {
        std::lock_guard lock(callbackMutex);
        if (!stopped && callback) callback(context,kind,label.c_str(),bytes,size);
    }
    void text(int kind,const std::string &label,const std::string &data) {
        emit(kind,label,reinterpret_cast<const uint8_t *>(data.data()),data.size());
    }
    void configure(const char *bind) {
        static std::once_flag init;
        std::call_once(init,[] {rtc::InitLogger(rtc::LogLevel::None);rtc::SetThreadPoolSize(4);});
        rtc::Configuration config;
        config.disableAutoNegotiation=true;config.forceMediaTransport=true;config.mtu=1280;
        if (bind && *bind) config.bindAddress=std::string(bind);
        pc=std::make_shared<rtc::PeerConnection>(config);
        const auto weak=weak_from_this();
        pc->onLocalDescription([weak](rtc::Description description) {
            if (auto self=weak.lock()) self->text(1,description.typeString(),std::string(description));
        });
        pc->onLocalCandidate([weak](rtc::Candidate candidate) {
            if (auto self=weak.lock()) self->text(2,candidate.mid(),std::string(candidate));
        });
        pc->onStateChange([weak](rtc::PeerConnection::State state) {
            if (auto self=weak.lock()) self->emit(5,std::to_string(int(state)));
        });
        pc->onTrack([weak](std::shared_ptr<rtc::Track> track) {
            if (auto self=weak.lock()) self->receive(std::move(track));
        });
        timer=std::thread([this] {
            while (!stopped) {
                std::this_thread::sleep_for(std::chrono::milliseconds(4));
                std::vector<std::shared_ptr<Receiver>> receivers;
                {std::lock_guard lock(tracksMutex);for (auto &[_,track]:tracks) if (track.receiver) receivers.push_back(track.receiver);}
                for (auto &receiver:receivers) receiver->tick();
            }
        });
    }
    static uint32_t window(const std::string &mid) {
        if (mid.empty() || mid.size()>10 || mid.find_first_not_of("0123456789")!=std::string::npos) throw std::runtime_error("invalid mid");
        auto value=std::stoull(mid);
        if (!value || value>UINT32_MAX) throw std::runtime_error("invalid window");
        return uint32_t(value);
    }
    void receive(std::shared_ptr<rtc::Track> channel) {
        if (sending) throw std::runtime_error("unexpected incoming media track");
        auto id=window(channel->mid());auto mid=std::to_string(id);auto weak=weak_from_this();
        auto receiver=std::make_shared<Receiver>(budget,[weak,mid](Frame frame) {
            if (auto self=weak.lock()) {
                if (frame.metadata.size()>MaxMetadata || frame.data.size()>MaxFrame) return;
                Bytes bytes(4+frame.metadata.size()+frame.data.size());
                const auto size=uint32_t(frame.metadata.size());
                for (int i=0;i<4;++i) bytes[i]=uint8_t(size>>(24-8*i));
                std::memcpy(bytes.data()+4,frame.metadata.data(),frame.metadata.size());
                std::memcpy(bytes.data()+4+frame.metadata.size(),frame.data.data(),frame.data.size());
                self->emit(3,mid,bytes.data(),bytes.size());
            }
        },[weak,mid] {if (auto self=weak.lock()) self->emit(7,mid);});
        channel->setMediaHandler(receiver);
        std::lock_guard lock(tracksMutex);
        if (tracks.size()>=64 || tracks.count(id)) throw std::runtime_error("track limit");
        tracks[id]={std::move(channel),std::move(receiver),0};
    }
    bool offer() {
        if (pc->signalingState()!=rtc::PeerConnection::SignalingState::Stable) return false;
        if (pc->negotiationNeeded()) pc->setLocalDescription(rtc::Description::Type::Offer);
        return true;
    }
    int add(uint32_t id) {
        if (!sending || !id) return -1;
        {std::lock_guard lock(tracksMutex);if (auto found=tracks.find(id);found!=tracks.end()) return found->second.channel->isOpen()?1:0;}
        auto mid=std::to_string(id);rtc::Description::Video description(mid,rtc::Description::Direction::SendOnly);
        description.addH264Codec(96,"profile-level-id=42e034;packetization-mode=1;level-asymmetry-allowed=1");
        description.addSSRC(id,"remoteapp",mid,mid);
        auto channel=pc->addTrack(description);
        auto config=std::make_shared<rtc::RtpPacketizationConfig>(id,"remoteapp",96,90000);
        auto packetizer=std::make_shared<rtc::H264RtpPacketizer>(rtc::NalUnit::Separator::StartSequence,config,1160);
        packetizer->addToChain(std::make_shared<rtc::RtcpSrReporter>(config));
        packetizer->addToChain(std::make_shared<rtc::RtcpNackResponder>(4096));
        auto weak=weak_from_this();
        packetizer->addToChain(std::make_shared<rtc::PliHandler>([weak,mid] {if (auto self=weak.lock()) self->emit(4,mid);}));
        channel->setMediaHandler(packetizer);
        channel->onOpen([weak,mid] {if (auto self=weak.lock()) self->emit(4,mid);});
        channel->onError([weak](std::string) {if (auto self=weak.lock()) self->emit(6,"track_failed");});
        {std::lock_guard lock(tracksMutex);if (tracks.size()>=64) throw std::runtime_error("track limit");tracks[id]={std::move(channel),nullptr,0};}
        offer();return 0;
    }
    void close() {
        {std::lock_guard lock(callbackMutex);stopped=true;callback=nullptr;context=nullptr;}
        if (timer.joinable()) timer.join();
        pc->close();
        {std::lock_guard lock(tracksMutex);for (auto &[_,track]:tracks) track.channel->close();tracks.clear();}
        pc.reset();
    }
};
using Handle=std::shared_ptr<Peer>;
Peer &peer(void *handle) {if (!handle) throw std::runtime_error("null peer");return **static_cast<Handle *>(handle);}
}
extern "C" {
void *ra_rtc_create(int sending,const char *bind,ra_rtc_callback callback,void *context) {
    try {
        auto value=std::make_shared<Peer>(sending!=0,callback,context);value->configure(bind);
        return new Handle(std::move(value));
    } catch (...) {return nullptr;}
}
int ra_rtc_add_video(void *handle,uint32_t id) {try {return peer(handle).add(id);} catch (...) {return -1;}}
int ra_rtc_description(void *handle,const char *sdp,const char *type) {
    try {
        if (!sdp || !type || std::strlen(sdp)>48*1024 || (std::string(type)!="offer" && std::string(type)!="answer")) return -1;
        auto &p=peer(handle);p.pc->setRemoteDescription(rtc::Description(sdp,type));
        if (!p.sending) p.pc->setLocalDescription(rtc::Description::Type::Answer);else p.offer();
        return 0;
    } catch (...) {return -1;}
}
int ra_rtc_candidate(void *handle,const char *candidate,const char *mid) {
    try {if (!candidate || !mid || std::strlen(candidate)>2048 || std::strlen(mid)>32) return -1;
        peer(handle).pc->addRemoteCandidate(rtc::Candidate(candidate,mid));return 0;
    } catch (...) {return -1;}
}
int ra_rtc_send(void *handle,uint32_t id,const uint8_t *data,size_t size,const uint8_t *metadata,size_t length) {
    try {
        auto &p=peer(handle);auto bytes=pack(data,size,metadata,length);
        std::lock_guard lock(p.tracksMutex);auto &track=p.tracks.at(id);
        if (!track.channel->isOpen()) return -1;
        track.timestamp+=3000;
        track.channel->sendFrame(reinterpret_cast<const rtc::byte *>(bytes.data()),bytes.size(),rtc::FrameInfo(track.timestamp));return 0;
    } catch (...) {return -1;}
}
int ra_rtc_request_keyframe(void *handle,uint32_t id) {
    try {
        std::shared_ptr<rtc::Track> channel;
        {auto &p=peer(handle);std::lock_guard lock(p.tracksMutex);channel=p.tracks.at(id).channel;}
        return channel->requestKeyframe()?0:-1;
    } catch (...) {return -1;}
}
int ra_rtc_selected_pair(void *handle,char *output,size_t size) {
    try {
        auto &p=peer(handle);rtc::Candidate local,remote;
        if (!p.pc->getSelectedCandidatePair(&local,&remote)) return -1;
        std::string value=std::string(local)+"\n"+std::string(remote);
        if (!output || value.size()+1>size) return -1;
        std::memcpy(output,value.c_str(),value.size()+1);return int(value.size());
    } catch (...) {return -1;}
}
void ra_rtc_close(void *handle) {
    if (!handle) return;
    try {peer(handle).close();} catch (...) {}
    delete static_cast<Handle *>(handle);
}
}
