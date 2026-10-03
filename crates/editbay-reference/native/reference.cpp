#include <torch/script.h>
#include <ATen/Parallel.h>
#include <cstring>
#include <memory>
#include <sstream>

struct Reference {
    torch::jit::Module model;
    std::vector<c10::IValue> states;
    explicit Reference(std::istream &input) : model(torch::jit::load(input, torch::kCPU)), states(4) {
        model.eval();
    }
};

static void failure(char *out, size_t length, const char *text) {
    if (length) { std::strncpy(out, text, length - 1); out[length - 1] = 0; }
}

extern "C" void *eb_reference_open(const unsigned char *bytes, size_t size, char *error, size_t length) {
    try {
        at::set_num_threads(2);
        std::istringstream input(std::string(reinterpret_cast<const char *>(bytes), size));
        return new Reference(input);
    } catch (const std::exception &e) { failure(error, length, e.what()); return nullptr; }
}

extern "C" int eb_reference_run(void *handle, const float *rgb, int width, int height,
    float ratio, float *alpha, float *foreground, char *error, size_t length) {
    try {
        torch::NoGradGuard guard;
        auto &ref = *static_cast<Reference *>(handle);
        auto input = torch::from_blob(const_cast<float *>(rgb), {1, 3, height, width}, torch::kFloat32);
        std::vector<c10::IValue> args = {input, ref.states[0], ref.states[1], ref.states[2], ref.states[3], double(ratio)};
        auto output = ref.model.forward(args).toTensorList();
        if (output.size() != 6) throw std::runtime_error("reference output count differs");
        auto fgr = output.get(0).contiguous();
        auto pha = output.get(1).contiguous();
        const auto pixels = static_cast<int64_t>(width) * height;
        if (fgr.numel() != 3 * pixels || pha.numel() != pixels || fgr.scalar_type() != torch::kFloat32
            || pha.scalar_type() != torch::kFloat32) throw std::runtime_error("reference output shape/type differs");
        std::memcpy(foreground, fgr.data_ptr<float>(), pixels * 3 * sizeof(float));
        std::memcpy(alpha, pha.data_ptr<float>(), pixels * sizeof(float));
        for (int i = 0; i < 4; ++i) ref.states[i] = output.get(i + 2);
        return 0;
    } catch (const std::exception &e) { failure(error, length, e.what()); return -1; }
}

extern "C" void eb_reference_close(void *handle) { delete static_cast<Reference *>(handle); }
