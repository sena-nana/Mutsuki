#ifndef MUTSUKI_PLUGIN_ABI_V1_H
#define MUTSUKI_PLUGIN_ABI_V1_H

#include <stddef.h>
#include <stdint.h>

#define MUTSUKI_PLUGIN_ABI_TRANSPORT_VERSION 1u
#define MUTSUKI_PLUGIN_ABI_ENTRY_SYMBOL "mutsuki_plugin_abi_v1"

typedef struct MutsukiAbiBuffer {
    uint8_t *ptr;
    size_t len;
} MutsukiAbiBuffer;

typedef struct MutsukiAbiCallResult {
    int32_t status;
    MutsukiAbiBuffer payload;
} MutsukiAbiCallResult;

typedef MutsukiAbiCallResult (*MutsukiAbiRequestFn)(
    void *context,
    const uint8_t *request,
    size_t request_len
);
typedef void (*MutsukiAbiReleaseFn)(MutsukiAbiBuffer buffer);
typedef void (*MutsukiAbiCloseFn)(void *context);

typedef struct MutsukiAbiHostV1 {
    void *context;
    MutsukiAbiRequestFn request;
    MutsukiAbiReleaseFn release;
} MutsukiAbiHostV1;

typedef struct MutsukiAbiPluginV1 {
    uint32_t transport_version;
    void *context;
    MutsukiAbiRequestFn request;
    MutsukiAbiReleaseFn release;
    MutsukiAbiCloseFn close;
} MutsukiAbiPluginV1;

MutsukiAbiPluginV1 mutsuki_plugin_abi_v1(MutsukiAbiHostV1 host);

#endif
