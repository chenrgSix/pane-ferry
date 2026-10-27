#pragma once
#include <stddef.h>
#include <stdint.h>
#if defined(__GNUC__)
#define RA_EXPORT __attribute__((visibility("default")))
#else
#define RA_EXPORT
#endif
#ifdef __cplusplus
extern "C" {
#endif
/* Callback data/label is borrowed only for the duration of the call.
 * 1: SDP (label=offer/answer), 2: ICE (label=mid), 3: complete video AU,
 * 4: sender needs a new keyframe (label=decimal window ID), 5: state,
 * 6: transport failure, 7: discarded incomplete media frame.
 * Complete video AU: big-endian u32 metadata size, JSON metadata, Annex-B H264.
 * All calls return <0 on failure; exceptions never cross this C ABI.
 */
typedef void (*ra_rtc_callback)(void *, int, const char *, const uint8_t *, size_t);
RA_EXPORT void *ra_rtc_create(int sending, const char *bind_address, ra_rtc_callback, void *);
RA_EXPORT int ra_rtc_add_video(void *, uint32_t window_id);
RA_EXPORT int ra_rtc_description(void *, const char *sdp, const char *type);
RA_EXPORT int ra_rtc_candidate(void *, const char *candidate, const char *mid);
RA_EXPORT int ra_rtc_send(void *, uint32_t window_id, const uint8_t *, size_t,
                          const uint8_t *metadata, size_t metadata_size);
RA_EXPORT int ra_rtc_request_keyframe(void *, uint32_t window_id);
RA_EXPORT int ra_rtc_selected_pair(void *, char *output, size_t size);
/* Close disables and drains callbacks before returning; user data may then be freed. */
RA_EXPORT void ra_rtc_close(void *);
#ifdef __cplusplus
}
#endif
