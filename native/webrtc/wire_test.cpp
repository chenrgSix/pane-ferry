#include "wire.hpp"
#include <cassert>
#include <iostream>
#include <string>
using namespace remoteapp;
template<class F> void rejected(F operation) {
    bool failed = false; try { operation(); } catch (...) { failed = true; } assert(failed);
}
int main() {
    Bytes input{0,0,0,1,0x67,42,0,0,3,1,0,0,1,0x68,1,2,3,0,0,0,1,0x65,4,5,6};
    std::string metadata = "{\"width\":2940,\"height\":1612,\"stream_id\":1}";
    auto encoded = pack(input.data(),input.size(),reinterpret_cast<const uint8_t *>(metadata.data()),metadata.size());
    auto frame = unpack(encoded.data(),encoded.size());
    assert(frame.data == annexb(nals(input.data(),input.size())) && frame.keyframe);
    assert(std::string(frame.metadata.begin(),frame.metadata.end()) == metadata);
    auto corrupted = encoded; corrupted.back() ^= 1;
    rejected([&] { unpack(corrupted.data(),corrupted.size()); });
    rejected([&] { unpack(encoded.data(),encoded.size()-1); });
    rejected([&] { pack(input.data(),input.size(),nullptr,0); });
    rejected([&] { unpack(input.data(),input.size()); });
    Bytes escapes{0,0,0,0,1,0,0,2,0,0,3,0,0,4,0xff};
    assert(unescape(escape(escapes)) == escapes);
    assert(extend16(0,65535)==65536 && extend16(65535,65536)==65535);
    assert(extend32(0,0xffffffffLL)==0x100000000LL);
    Bytes rtp{0x80,0xe0,0xff,0xff,0,0,0,7,0,0,0,9,0x65,1};
    auto packet = RTP::parse(rtp.data(),rtp.size());
    assert(packet.marker && packet.sequence==65535 && packet.timestamp==7 && packet.ssrc==9);
    rtp[0] |= 16; rejected([&] { RTP::parse(rtp.data(),rtp.size()); });
    rtp.resize(2); rejected([&] { RTP::parse(rtp.data(),rtp.size()); });
    std::cout << "PASS: loss detection, SEI metadata, unchanged NALs, escaping, wrap and bounded RTP parsing\n";
}
