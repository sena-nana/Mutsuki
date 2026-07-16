#ifndef MUTSUKI_PLUGIN_ABI_V2_H
#define MUTSUKI_PLUGIN_ABI_V2_H

#include <stddef.h>
#include <stdint.h>

#define MUTSUKI_PLUGIN_ABI_V2_TRANSPORT_VERSION 2u
#define MUTSUKI_PLUGIN_ABI_V2_ENTRY_SYMBOL "mutsuki_plugin_abi_v2"
#define MUTSUKI_PLUGIN_ABI_V2_CODEC_ID "mutsuki.codec.typed-msgpack.v1"

typedef struct MutsukiAbiBufferV2 {
    uint8_t *ptr;
    size_t len;
} MutsukiAbiBufferV2;

typedef struct MutsukiAbiCallResultV2 {
    int32_t status;
    MutsukiAbiBufferV2 payload;
} MutsukiAbiCallResultV2;

typedef MutsukiAbiCallResultV2 (*MutsukiAbiRequestFnV2)(
    void *context,
    const uint8_t *request,
    size_t request_len
);
typedef void (*MutsukiAbiReleaseFnV2)(MutsukiAbiBufferV2 buffer);
typedef void (*MutsukiAbiCloseFnV2)(void *context);

typedef struct MutsukiAbiHostV2 {
    void *context;
    MutsukiAbiRequestFnV2 request;
    MutsukiAbiReleaseFnV2 release;
} MutsukiAbiHostV2;

typedef struct MutsukiAbiPluginV2 {
    uint32_t transport_version;
    void *context;
    MutsukiAbiRequestFnV2 request;
    MutsukiAbiReleaseFnV2 release;
    MutsukiAbiCloseFnV2 close;
} MutsukiAbiPluginV2;

MutsukiAbiPluginV2 mutsuki_plugin_abi_v2(MutsukiAbiHostV2 host);

#endif
