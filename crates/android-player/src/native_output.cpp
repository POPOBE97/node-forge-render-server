// Public Android SurfaceControl/AHardwareBuffer HDR output. The normalized P3
// protocol follows p3-vulkan-desktop; no device-specific DSPP calibration is used.
#include <android/hardware_buffer.h>
#include <android/surface_control.h>
#include <vulkan/vulkan.h>
#include <vulkan/vulkan_android.h>
#include <dlfcn.h>
#include <poll.h>
#include <unistd.h>
#include <array>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>

namespace {
void check(VkResult result) {
    if (result != VK_SUCCESS) throw std::runtime_error("Vulkan HDR output: " + std::to_string(result));
}
struct Slot { AHardwareBuffer* buffer = nullptr; bool available = true; int fence = -1; };
struct Pool {
    ASurfaceControl* control = nullptr;
    std::array<Slot, 3> slots{};
    std::mutex mutex;
    std::condition_variable changed;
    ~Pool() {
        for (auto& slot : slots) {
            if (slot.fence >= 0) close(slot.fence);
            if (slot.buffer) AHardwareBuffer_release(slot.buffer);
        }
        if (control) ASurfaceControl_release(control);
    }
};
struct Completion { std::shared_ptr<Pool> pool; int previous; };
void completed(void* context, ASurfaceTransactionStats* stats) {
    std::unique_ptr<Completion> cb(static_cast<Completion*>(context));
    if (cb->previous < 0) return;
    int fd = ASurfaceTransactionStats_getPreviousReleaseFenceFd(stats, cb->pool->control);
    {
        std::lock_guard<std::mutex> lock(cb->pool->mutex);
        auto& slot = cb->pool->slots[cb->previous];
        slot.fence = fd;
        slot.available = true;
    }
    cb->pool->changed.notify_all();
}
struct Imported { VkImage image = VK_NULL_HANDLE; VkDeviceMemory memory = VK_NULL_HANDLE; bool used = false; };

class Output {
    using SetRange = void (*)(ASurfaceTransaction*, ASurfaceControl*, float, float);
    SetRange setRange = nullptr;
    std::shared_ptr<Pool> pool = std::make_shared<Pool>();
    std::array<Imported, 3> images{};
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    VkCommandPool commands = VK_NULL_HANDLE;
    VkCommandBuffer command = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    VkExtent3D extent{};
    uint32_t family = 0;
    int current = -1;
    bool failed = false;
public:
    void invalidate() { failed = true; }
    ~Output() {
        if (device) vkDeviceWaitIdle(device);
        if (pool->control) {
            auto* transaction = ASurfaceTransaction_create();
            ASurfaceTransaction_setBuffer(transaction, pool->control, nullptr, -1);
            ASurfaceTransaction_setVisibility(transaction, pool->control, ASURFACE_TRANSACTION_VISIBILITY_HIDE);
            ASurfaceTransaction_reparent(transaction, pool->control, nullptr);
            ASurfaceTransaction_setOnComplete(transaction, new Completion{pool, current}, completed);
            ASurfaceTransaction_apply(transaction);
            ASurfaceTransaction_delete(transaction);
        }
        // Compositor references and release callbacks outlive this object. GPU
        // imports can be destroyed once our queue is idle; buffers remain in Pool.
        for (auto& image : images) {
            if (image.image) vkDestroyImage(device, image.image, nullptr);
            if (image.memory) vkFreeMemory(device, image.memory, nullptr);
        }
        if (fence) vkDestroyFence(device, fence, nullptr);
        if (commands) vkDestroyCommandPool(device, commands, nullptr);
    }

    void initialize(ANativeWindow* window, VkPhysicalDevice physical, VkDevice dev,
                    VkQueue renderQueue, uint32_t queueFamily, uint32_t width, uint32_t height) {
        device = dev; queue = renderQueue; family = queueFamily; extent = {width, height, 1};
        setRange = reinterpret_cast<SetRange>(dlsym(RTLD_DEFAULT, "ASurfaceTransaction_setExtendedRangeBrightness"));
        if (!setRange) throw std::runtime_error("Android extended-range brightness API unavailable");
        auto getProperties = reinterpret_cast<PFN_vkGetAndroidHardwareBufferPropertiesANDROID>(
            vkGetDeviceProcAddr(device, "vkGetAndroidHardwareBufferPropertiesANDROID"));
        if (!getProperties) throw std::runtime_error("Vulkan AHardwareBuffer import unavailable");
        pool->control = ASurfaceControl_createFromWindow(window, "Node Forge HDR P3");
        if (!pool->control) throw std::runtime_error("Cannot create Android HDR layer");
        VkPhysicalDeviceMemoryProperties memory{};
        vkGetPhysicalDeviceMemoryProperties(physical, &memory);
        for (size_t n = 0; n < images.size(); ++n) {
            AHardwareBuffer_Desc desc{};
            desc.width = width; desc.height = height; desc.layers = 1;
            desc.format = AHARDWAREBUFFER_FORMAT_R10G10B10A2_UNORM;
            desc.usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE
                | AHARDWAREBUFFER_USAGE_COMPOSER_OVERLAY;
            int result = AHardwareBuffer_allocate(&desc, &pool->slots[n].buffer);
            if (result) throw std::runtime_error("HDR AHardwareBuffer allocation: " + std::to_string(result));
            VkAndroidHardwareBufferPropertiesANDROID props{}; props.sType = VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_PROPERTIES_ANDROID;
            check(getProperties(device, pool->slots[n].buffer, &props));
            VkExternalMemoryImageCreateInfo external{}; external.sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;
            external.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID;
            VkImageCreateInfo info{}; info.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
            info.pNext = &external; info.imageType = VK_IMAGE_TYPE_2D;
            info.format = VK_FORMAT_A2B10G10R10_UNORM_PACK32; info.extent = extent;
            info.mipLevels = 1; info.arrayLayers = 1; info.samples = VK_SAMPLE_COUNT_1_BIT;
            info.tiling = VK_IMAGE_TILING_OPTIMAL;
            info.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
            info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
            check(vkCreateImage(device, &info, nullptr, &images[n].image));
            uint32_t type = 0;
            for (; type < memory.memoryTypeCount; ++type) {
                if ((props.memoryTypeBits & (1u << type)) &&
                    (memory.memoryTypes[type].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) break;
            }
            if (type == memory.memoryTypeCount) throw std::runtime_error("No device-local HDR buffer memory");
            VkMemoryDedicatedAllocateInfo dedicated{}; dedicated.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;
            dedicated.image = images[n].image;
            VkImportAndroidHardwareBufferInfoANDROID ahb{}; ahb.sType = VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID;
            ahb.buffer = pool->slots[n].buffer; ahb.pNext = &dedicated;
            VkMemoryAllocateInfo alloc{}; alloc.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
            alloc.pNext = &ahb; alloc.allocationSize = props.allocationSize; alloc.memoryTypeIndex = type;
            check(vkAllocateMemory(device, &alloc, nullptr, &images[n].memory));
            check(vkBindImageMemory(device, images[n].image, images[n].memory, 0));
        }
        VkCommandPoolCreateInfo cmdPool{}; cmdPool.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
        cmdPool.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT; cmdPool.queueFamilyIndex = family;
        check(vkCreateCommandPool(device, &cmdPool, nullptr, &commands));
        VkCommandBufferAllocateInfo cmd{}; cmd.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
        cmd.commandPool = commands; cmd.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; cmd.commandBufferCount = 1;
        check(vkAllocateCommandBuffers(device, &cmd, &command));
        VkFenceCreateInfo fenceInfo{}; fenceInfo.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
        check(vkCreateFence(device, &fenceInfo, nullptr, &fence));
    }

    void present(VkImage source, float ratio) {
        if (failed) throw std::runtime_error("Android HDR output failed; reconnect the Player");
        int n = 0;
        {
            std::unique_lock<std::mutex> lock(pool->mutex);
            bool ready = pool->changed.wait_for(lock, std::chrono::seconds(2), [&] {
                for (auto& slot : pool->slots) if (slot.available) return true;
                return false;
            });
            if (!ready) throw std::runtime_error("Timed out waiting for Android HDR buffer release");
            while (!pool->slots[n].available) ++n;
            auto& slot = pool->slots[n]; slot.available = false;
            int fd = slot.fence; slot.fence = -1; lock.unlock();
            if (fd >= 0) {
                pollfd p{fd, POLLIN, 0}; int result;
                do { result = poll(&p, 1, 2000); } while (result < 0 && errno == EINTR);
                close(fd);
                if (result <= 0 || !(p.revents & POLLIN)) throw std::runtime_error("HDR compositor release fence did not signal");
            }
        }
        try {
            auto& image = images[n];
            check(vkResetCommandPool(device, commands, 0));
            check(vkResetFences(device, 1, &fence));
            VkCommandBufferBeginInfo begin{}; begin.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
            begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
            check(vkBeginCommandBuffer(command, &begin));
            VkImageMemoryBarrier barrier{}; barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.image = image.image; barrier.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
            barrier.oldLayout = image.used ? VK_IMAGE_LAYOUT_GENERAL : VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.srcQueueFamilyIndex = image.used ? VK_QUEUE_FAMILY_FOREIGN_EXT : VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = image.used ? family : VK_QUEUE_FAMILY_IGNORED;
            vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);
            // wgpu explicitly transitions source to COPY_SRC and waits before calling us.
            VkImageCopy region{}; region.srcSubresource = region.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.extent = extent;
            vkCmdCopyImage(command, source, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                           image.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; barrier.newLayout = VK_IMAGE_LAYOUT_GENERAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; barrier.dstAccessMask = 0;
            barrier.srcQueueFamilyIndex = family; barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_FOREIGN_EXT;
            vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);
            check(vkEndCommandBuffer(command));
            VkSubmitInfo submit{}; submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
            submit.commandBufferCount = 1; submit.pCommandBuffers = &command;
            check(vkQueueSubmit(queue, 1, &submit, fence));
            image.used = true;
            check(vkWaitForFences(device, 1, &fence, VK_TRUE, 2'000'000'000));
        } catch (...) {
            // A submitted copy may still be in flight after timeout. Finish it
            // before the buffer/command pool can be reused or destroyed.
            vkDeviceWaitIdle(device);
            std::lock_guard<std::mutex> lock(pool->mutex);
            pool->slots[n].available = true;
            throw;
        }
        auto* transaction = ASurfaceTransaction_create();
        ASurfaceTransaction_setBuffer(transaction, pool->control, pool->slots[n].buffer, -1);
        // STANDARD_DCI_P3 | TRANSFER_SRGB | RANGE_EXTENDED, public Android dataspace bits.
        ASurfaceTransaction_setBufferDataSpace(transaction, pool->control, static_cast<ADataSpace>(0x188a0000));
        ASurfaceTransaction_setBufferTransparency(transaction, pool->control, ASURFACE_TRANSACTION_TRANSPARENCY_OPAQUE);
        ASurfaceTransaction_setVisibility(transaction, pool->control, ASURFACE_TRANSACTION_VISIBILITY_SHOW);
        ASurfaceTransaction_setZOrder(transaction, pool->control, 1);
        setRange(transaction, pool->control, ratio, ratio);
        ASurfaceTransaction_setOnComplete(transaction, new Completion{pool, current}, completed);
        current = n;
        ASurfaceTransaction_apply(transaction); ASurfaceTransaction_delete(transaction);
    }
};
void errorText(char* out, size_t size, const std::exception& error) {
    snprintf(out, size, "%s", error.what());
}
} // namespace

extern "C" void* nf_hdr_create(ANativeWindow* window, VkPhysicalDevice physical, VkDevice device,
                               VkQueue queue, uint32_t family, uint32_t width, uint32_t height,
                               char* error, size_t size) {
    try {
        auto output = std::make_unique<Output>();
        output->initialize(window, physical, device, queue, family, width, height);
        return output.release();
    } catch (const std::exception& e) { errorText(error, size, e); return nullptr; }
}
extern "C" bool nf_hdr_present(void* output, VkImage source, float ratio, char* error, size_t size) {
    try { static_cast<Output*>(output)->present(source, ratio); return true; }
    catch (const std::exception& e) { static_cast<Output*>(output)->invalidate(); errorText(error, size, e); return false; }
}
extern "C" void nf_hdr_destroy(void* output) { delete static_cast<Output*>(output); }
