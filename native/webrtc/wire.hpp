#pragma once
#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <vector>

namespace remoteapp {
using Bytes = std::vector<uint8_t>;
constexpr size_t MaxFrame = 12 * 1024 * 1024;
constexpr size_t MaxMetadata = 4096;
constexpr std::array<uint8_t, 16> UUID = {'L','A','N','R','e','m','o','t','e','A','p','p','0','0','0','1'};
inline uint16_t u16(const uint8_t *p) { return uint16_t(p[0]) << 8 | p[1]; }
inline uint32_t u32(const uint8_t *p) {
    return uint32_t(p[0]) << 24 | uint32_t(p[1]) << 16 | uint32_t(p[2]) << 8 | p[3];
}
inline void put32(Bytes &out, uint32_t value) {
    for (int shift = 24; shift >= 0; shift -= 8) out.push_back(uint8_t(value >> shift));
}
inline uint32_t crc32(const Bytes &bytes) {
    static const auto table = [] {
        std::array<uint32_t,256> result{};
        for (uint32_t n=0;n<256;++n) {
            uint32_t v=n;
            for (int bit=0;bit<8;++bit) v=(v>>1)^(0xedb88320u & (0u-(v&1)));
            result[n]=v;
        }
        return result;
    }();
    uint32_t crc = ~uint32_t(0);
    for (uint8_t byte : bytes) crc=(crc>>8)^table[(crc^byte)&255];
    return ~crc;
}
inline std::vector<Bytes> nals(const uint8_t *data, size_t size) {
    if (!data || size < 4 || size > MaxFrame) throw std::runtime_error("invalid access unit");
    std::vector<Bytes> result;
    size_t start = 0;
    auto separator = [&](size_t offset) -> size_t {
        if (offset + 3 > size || data[offset] || data[offset+1]) return 0;
        if (data[offset+2] == 1) return 3;
        return offset + 4 <= size && !data[offset+2] && data[offset+3] == 1 ? 4 : 0;
    };
    while (start < size) {
        const size_t prefix = separator(start);
        if (!prefix) throw std::runtime_error("missing Annex-B separator");
        const size_t begin = start + prefix;
        size_t end = begin;
        while (end < size && !separator(end)) ++end;
        if (end == begin || (data[begin] & 0x80) || result.size() == 1024)
            throw std::runtime_error("invalid NAL unit");
        result.emplace_back(data + begin, data + end);
        start = end;
    }
    return result;
}
inline Bytes annexb(const std::vector<Bytes> &units) {
    Bytes out;
    for (const auto &unit : units) {
        out.insert(out.end(), {0,0,0,1}); out.insert(out.end(), unit.begin(), unit.end());
    }
    return out;
}
inline Bytes escape(const Bytes &rbsp) {
    Bytes result; int zeros = 0;
    for (uint8_t byte : rbsp) {
        if (zeros >= 2 && byte <= 3) { result.push_back(3); zeros = 0; }
        result.push_back(byte); zeros = byte == 0 ? zeros + 1 : 0;
    }
    return result;
}
inline Bytes unescape(const Bytes &ebsp) {
    Bytes result; int zeros = 0;
    for (size_t i = 0; i < ebsp.size(); ++i) {
        uint8_t byte = ebsp[i];
        if (zeros >= 2 && byte == 3) {
            if (i+1 == ebsp.size() || ebsp[i+1] > 3) throw std::runtime_error("invalid emulation prevention");
            zeros = 0; continue;
        }
        result.push_back(byte); zeros = byte == 0 ? zeros + 1 : 0;
    }
    return result;
}
/* Unregistered SEI carries geometry/session metadata in the SAME SRTP media AU.
 * No per-frame control side channel, transcoding or pixel changes. CRC detects an
 * incomplete reassembly; authentication is provided by DTLS-SRTP, not this CRC. */
inline Bytes pack(const uint8_t *data, size_t size, const uint8_t *metadata, size_t length) {
    if (!metadata || !length || length > MaxMetadata) throw std::runtime_error("invalid metadata");
    auto units = nals(data, size);
    const auto normalized = annexb(units);
    Bytes payload(UUID.begin(), UUID.end());
    put32(payload, uint32_t(normalized.size())); put32(payload, crc32(normalized));
    payload.insert(payload.end(), metadata, metadata + length);
    Bytes sei{5};
    size_t remaining = payload.size();
    while (remaining >= 255) { sei.push_back(255); remaining -= 255; }
    sei.push_back(uint8_t(remaining)); sei.insert(sei.end(), payload.begin(), payload.end()); sei.push_back(0x80);
    auto escaped = escape(sei); escaped.insert(escaped.begin(), 6);
    units.insert(units.begin(), std::move(escaped));
    auto result = annexb(units);
    if (result.size() > MaxFrame) throw std::runtime_error("oversized media access unit");
    return result;
}
struct Frame { Bytes metadata, data; bool keyframe = false; };
inline Frame unpack(const uint8_t *data, size_t size) {
    auto units = nals(data, size); Frame frame;
    uint32_t expectedSize = 0, expectedCRC = 0;
    std::vector<Bytes> original;
    for (const auto &unit : units) {
        bool ours = false;
        if ((unit[0] & 31) == 6) {
            Bytes ebsp(unit.begin()+1, unit.end()); auto rbsp = unescape(ebsp);
            size_t offset = 0;
            while (offset + 1 < rbsp.size()) {
                size_t type = 0, length = 0;
                while (offset < rbsp.size() && rbsp[offset] == 255) { type += 255; ++offset; }
                if (offset == rbsp.size()) break;
                type += rbsp[offset++];
                while (offset < rbsp.size() && rbsp[offset] == 255) { length += 255; ++offset; }
                if (offset == rbsp.size()) break;
                length += rbsp[offset++];
                if (length > rbsp.size() - offset) throw std::runtime_error("truncated SEI");
                if (type == 5 && length >= UUID.size()+8 &&
                    std::equal(UUID.begin(), UUID.end(), rbsp.begin()+offset)) {
                    if (!frame.metadata.empty() || length > MaxMetadata+24) throw std::runtime_error("duplicate metadata");
                    expectedSize = u32(&rbsp[offset+16]); expectedCRC = u32(&rbsp[offset+20]);
                    frame.metadata.assign(rbsp.begin()+offset+24, rbsp.begin()+offset+length);
                    ours = true;
                    break;
                }
                offset += length;
            }
        }
        if (ours) continue;
        if ((unit[0] & 31) == 5) frame.keyframe = true;
        original.push_back(unit);
    }
    frame.data = annexb(original);
    if (frame.metadata.empty() || frame.metadata.size() > MaxMetadata || expectedSize != frame.data.size() ||
        crc32(frame.data) != expectedCRC) throw std::runtime_error("incomplete media frame");
    return frame;
}
struct RTP {
    uint16_t sequence; uint32_t timestamp, ssrc; bool marker;
    size_t payloadOffset;
    static RTP parse(const uint8_t *data, size_t size) {
        if (!data || size < 12 || size > 2048 || data[0] >> 6 != 2 || (data[1] & 127) != 96)
            throw std::runtime_error("invalid RTP packet");
        size_t header = 12 + (data[0] & 15)*4;
        if (header > size) throw std::runtime_error("truncated CSRC");
        if (data[0] & 16) {
            if (header+4 > size) throw std::runtime_error("truncated extension");
            header += 4 + size_t(u16(data+header+2))*4;
        }
        size_t padding = data[0] & 32 ? data[size-1] : 0;
        if (padding > size || header >= size-padding) throw std::runtime_error("empty RTP payload");
        return {u16(data+2),u32(data+4),u32(data+8),bool(data[1]&128),header};
    }
};
inline int64_t extend16(uint16_t value, int64_t reference) {
    return reference + int16_t(value - uint16_t(reference));
}
inline int64_t extend32(uint32_t value, int64_t reference) {
    return reference + int32_t(value - uint32_t(reference));
}
} // namespace remoteapp
