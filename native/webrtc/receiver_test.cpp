#include "receiver.hpp"
#include <cassert>
#include <iostream>
#include <thread>
using namespace remoteapp;
int main() {
    auto config=std::make_shared<rtc::RtpPacketizationConfig>(7,"test",96,90000);
    config->sequenceNumber=65530;
    rtc::H264RtpPacketizer packetizer(rtc::NalUnit::Separator::StartSequence,config,1160);
    auto budget=std::make_shared<Budget>();
    int losses=0;std::vector<Frame> delivered;std::vector<Bytes> feedback;
    Receiver receiver(budget,[&](Frame frame) {delivered.push_back(std::move(frame));},[&] {++losses;});
    auto send=[&](rtc::message_ptr packet) {
        feedback.emplace_back(reinterpret_cast<const uint8_t *>(packet->data()),reinterpret_cast<const uint8_t *>(packet->data())+packet->size());
    };
    Bytes original;
    auto packets=[&](bool key,uint32_t timestamp) {
        original={0,0,0,1,uint8_t(key?0x65:0x61)};original.resize(40000,0x55);
        const uint8_t meta[]={'{','}'};
        auto data=pack(original.data(),original.size(),meta,2);
        auto message=rtc::make_message(data.size(),rtc::Message::Binary);
        std::memcpy(message->data(),data.data(),data.size());message->frameInfo=std::make_shared<rtc::FrameInfo>(timestamp);
        rtc::message_vector messages{message};packetizer.outgoingChain(messages,send);return messages;
    };
    auto input=[&](const rtc::message_ptr &packet) {rtc::message_vector list{packet};receiver.incoming(list,send);assert(list.empty());};
    auto first=packets(true,3000);
    for (auto it=first.rbegin();it!=first.rend();++it) {input(*it);input(*it);}
    assert(delivered.size()==1 && delivered.back().data==original && budget->used==0);
    auto second=packets(false,6000);
    for (size_t i=0;i<second.size();++i) if (i!=5) input(second[i]);
    assert(delivered.size()==1);
    std::this_thread::sleep_for(std::chrono::milliseconds(4));receiver.tick();
    bool nacked=false;
    const auto missing=RTP::parse(reinterpret_cast<const uint8_t *>(second[5]->data()),second[5]->size()).sequence;
    for (const auto &packet:feedback) if (packet.size()>=16 && packet[1]==205 && u16(packet.data()+12)==missing) nacked=true;
    assert(nacked);input(second[5]);
    assert(delivered.size()==2 && delivered.back().data==original && budget->used==0);
    auto third=packets(false,9000);
    for (size_t i=0;i<third.size();++i) if (i!=5) input(third[i]);
    std::this_thread::sleep_for(std::chrono::milliseconds(70));receiver.tick();
    assert(delivered.size()==2 && losses>0 && budget->used==0);
    auto fourth=packets(false,12000);for (const auto &packet:fourth) input(packet);
    assert(delivered.size()==2);
    auto fifth=packets(true,15000);for (const auto &packet:fifth) input(packet);
    assert(delivered.size()==3 && delivered.back().data==original && budget->used==0);
    std::cout << "PASS: reordered/duplicate RTP, sequence wrap, NACK repair, deadline PLI, keyframe recovery and storage release\n";
}
