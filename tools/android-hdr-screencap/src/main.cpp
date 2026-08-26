// Derived from AOSP frameworks/base/cmds/screencap (Apache 2.0).
// Fingerprint-locked to the measured Android 16 nubia ROM. See PROVENANCE.md.

#include <binder/ProcessState.h>
#include <gui/SurfaceComposerClient.h>
#include <gui/SyncScreenCaptureListener.h>
#include <ui/DisplayState.h>
#include <ui/GraphicBuffer.h>
#include <ui/PixelFormat.h>
#include <utils/Errors.h>

#include <android/log.h>

#include <cctype>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cerrno>
#include <string>
#include <vector>

#include <unistd.h>

using namespace android;

namespace {

constexpr uint8_t kMagic[8] = {'N', 'F', 'R', 'C', 'L', 'I', 'P', 0};
constexpr uint16_t kVersion = 1;
constexpr uint16_t kHeaderLen = 64;
constexpr uint32_t kFlagHdr = 1u << 0;
constexpr uint32_t kFlagPng = 1u << 1;
constexpr uint16_t kKindLinearRgba16F = 1;
constexpr uint16_t kTransferLinearScRgb = 1;
constexpr uint16_t kAlphaPremultiplied = 1;

void write_le_u16(std::vector<uint8_t>* out, uint16_t value) {
    out->push_back(uint8_t(value));
    out->push_back(uint8_t(value >> 8));
}

void write_le_u32(std::vector<uint8_t>* out, uint32_t value) {
    out->push_back(uint8_t(value));
    out->push_back(uint8_t(value >> 8));
    out->push_back(uint8_t(value >> 16));
    out->push_back(uint8_t(value >> 24));
}

void write_le_u64(std::vector<uint8_t>* out, uint64_t value) {
    write_le_u32(out, uint32_t(value));
    write_le_u32(out, uint32_t(value >> 32));
}

bool is_linear_scrbg(ui::Dataspace dataspace) {
    const auto standard = ui::Dataspace(int32_t(dataspace) & int32_t(ui::Dataspace::STANDARD_MASK));
    const auto transfer = ui::Dataspace(int32_t(dataspace) & int32_t(ui::Dataspace::TRANSFER_MASK));
    const auto range = ui::Dataspace(int32_t(dataspace) & int32_t(ui::Dataspace::RANGE_MASK));
    return standard == ui::Dataspace::STANDARD_BT709 && transfer == ui::Dataspace::TRANSFER_LINEAR &&
           (range == ui::Dataspace::RANGE_EXTENDED || dataspace == ui::Dataspace::V0_SCRGB_LINEAR);
}

bool rgba16f_is_finite(const std::vector<uint8_t>& packed) {
    if ((packed.size() & 1u) != 0) {
        return false;
    }
    for (size_t offset = 0; offset < packed.size(); offset += 2) {
        uint16_t bits = 0;
        memcpy(&bits, packed.data() + offset, sizeof(bits));
        if ((bits & 0x7c00u) == 0x7c00u) {
            return false;
        }
    }
    return true;
}

struct Capture {
    uint32_t width = 0;
    uint32_t height = 0;
    bool capturedHdrLayers = false;
    std::vector<uint8_t> packedRgba16f;
};

status_t capture_fp16(Capture* out) {
    const auto displayIds = SurfaceComposerClient::getPhysicalDisplayIds();
    if (displayIds.empty()) {
        fprintf(stderr, "node-forge-hdr-screencap: no physical display\n");
        return NAME_NOT_FOUND;
    }
    const auto displayId = displayIds.front();
    const auto displayToken = SurfaceComposerClient::getPhysicalDisplayToken(displayId);
    ui::DisplayState displayState;
    status_t err = SurfaceComposerClient::getDisplayState(displayToken, &displayState);
    if (err != NO_ERROR) {
        fprintf(stderr, "node-forge-hdr-screencap: getDisplayState failed %d\n", err);
        return err;
    }
    (void)displayState.orientation;

    DisplayCaptureArgs args;
    args.displayToken = displayToken;
    args.pixelFormat = ui::PixelFormat::RGBA_FP16;
    args.dataspace = ui::Dataspace::V0_SCRGB_LINEAR;
    args.sourceCrop = {};
    args.width = 0;
    args.height = 0;
    args.useIdentityTransform = false;
    args.captureSecureLayers = false;

    sp<SyncScreenCaptureListener> listener = new SyncScreenCaptureListener();
    err = ScreenshotClient::captureDisplay(args, listener);
    if (err != NO_ERROR) {
        fprintf(stderr, "node-forge-hdr-screencap: captureDisplay failed %d\n", err);
        return err;
    }
    ScreenCaptureResults results = listener->waitForResults();
    if (!results.fenceResult.ok()) {
        fprintf(stderr, "node-forge-hdr-screencap: capture fence failed\n");
        return UNKNOWN_ERROR;
    }
    if (results.fenceResult.value() != Fence::NO_FENCE) {
        err = results.fenceResult.value()->waitForever("node-forge-hdr-screencap");
        if (err != NO_ERROR) {
            fprintf(stderr, "node-forge-hdr-screencap: fence wait failed %d\n", err);
            return err;
        }
    }
    if (results.buffer == nullptr) {
        fprintf(stderr, "node-forge-hdr-screencap: missing GraphicBuffer\n");
        return UNKNOWN_ERROR;
    }
    if (results.buffer->getPixelFormat() != PIXEL_FORMAT_RGBA_FP16) {
        fprintf(stderr, "node-forge-hdr-screencap: unexpected pixel format %d\n",
                results.buffer->getPixelFormat());
        return BAD_VALUE;
    }
    if (!is_linear_scrbg(results.capturedDataspace)) {
        fprintf(stderr, "node-forge-hdr-screencap: unexpected dataspace 0x%x\n",
                int(results.capturedDataspace));
        return BAD_VALUE;
    }

    void* base = nullptr;
    err = results.buffer->lock(GraphicBuffer::USAGE_SW_READ_OFTEN, &base);
    if (err != NO_ERROR || base == nullptr) {
        fprintf(stderr, "node-forge-hdr-screencap: lock failed %d\n", err);
        if (err == NO_ERROR) {
            results.buffer->unlock();
        }
        return err != NO_ERROR ? err : UNKNOWN_ERROR;
    }
    const uint32_t width = results.buffer->getWidth();
    const uint32_t height = results.buffer->getHeight();
    const uint32_t stride = results.buffer->getStride();
    out->width = width;
    out->height = height;
    out->capturedHdrLayers = results.capturedHdrLayers;
    out->packedRgba16f.resize(size_t(width) * size_t(height) * 8);
    for (uint32_t y = 0; y < height; ++y) {
        memcpy(out->packedRgba16f.data() + size_t(y) * width * 8,
               static_cast<uint8_t*>(base) + size_t(y) * stride * 8, size_t(width) * 8);
    }
    results.buffer->unlock();
    if (!rgba16f_is_finite(out->packedRgba16f)) {
        fprintf(stderr, "node-forge-hdr-screencap: capture contains non-finite FP16 pixels\n");
        out->packedRgba16f.clear();
        return BAD_VALUE;
    }
    return NO_ERROR;
}

std::vector<uint8_t> stock_png() {
    FILE* pipe = popen("/system/bin/screencap -p", "r");
    if (pipe == nullptr) {
        fprintf(stderr, "node-forge-hdr-screencap: stock screencap unavailable\n");
        return {};
    }
    std::vector<uint8_t> png;
    uint8_t buf[4096];
    while (true) {
        size_t n = fread(buf, 1, sizeof(buf), pipe);
        if (n == 0) {
            break;
        }
        png.insert(png.end(), buf, buf + n);
    }
    int status = pclose(pipe);
    if (status != 0 || png.size() < 8 || png[0] != 0x89 || png[1] != 'P') {
        fprintf(stderr, "node-forge-hdr-screencap: stock PNG encode failed status=%d bytes=%zu\n",
                status, png.size());
        return {};
    }
    return png;
}

void write_envelope(const Capture& capture, const std::vector<uint8_t>& png) {
    std::vector<uint8_t> out;
    out.reserve(64 + capture.packedRgba16f.size() + png.size());
    out.insert(out.end(), kMagic, kMagic + 8);
    write_le_u16(&out, kVersion);
    write_le_u16(&out, kHeaderLen);
    uint32_t flags = 0;
    if (capture.capturedHdrLayers) {
        flags |= kFlagHdr;
    }
    if (!png.empty()) {
        flags |= kFlagPng;
    }
    write_le_u32(&out, flags);
    write_le_u16(&out, kKindLinearRgba16F);
    write_le_u16(&out, kTransferLinearScRgb);
    write_le_u16(&out, kAlphaPremultiplied);
    write_le_u16(&out, 0);
    write_le_u32(&out, capture.width);
    write_le_u32(&out, capture.height);
    write_le_u32(&out, capture.width * 8);
    write_le_u32(&out, 0);
    write_le_u64(&out, capture.packedRgba16f.size());
    write_le_u64(&out, png.size());
    write_le_u64(&out, 0);
    out.insert(out.end(), capture.packedRgba16f.begin(), capture.packedRgba16f.end());
    out.insert(out.end(), png.begin(), png.end());
    fwrite(out.data(), 1, out.size(), stdout);
    fflush(stdout);
}

}  // namespace

int main() {
    if (getpgrp() != getpid() && setpgid(0, 0) != 0) {
        fprintf(stderr, "node-forge-hdr-screencap: setpgid failed errno=%d\n", errno);
        return 1;
    }
    ProcessState::self()->setThreadPoolMaxThreadCount(0);
    ProcessState::self()->startThreadPool();

    Capture capture;
    status_t err = capture_fp16(&capture);
    if (err != NO_ERROR) {
        return 1;
    }
    std::vector<uint8_t> png = stock_png();
    write_envelope(capture, png);
    return 0;
}
