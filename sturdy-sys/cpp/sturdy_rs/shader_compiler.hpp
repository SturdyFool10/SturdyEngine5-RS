// Rust <-> SturdyEngine 5 Slang-shader-compiler shim. See src/shader_compiler.rs for why this
// exists (making the raw RHI pipeline path testable from Rust, distinct from
// sturdy::assets::Assets::load_shader's file-based, Asset-producing path).
#pragma once

#include <cstdint>

#include <rust/cxx.h>

namespace sturdy_rs::shader_compiler {

    enum class ShaderStage : std::uint8_t;
    struct EntryPointRequest;
    struct ShaderCompileResult;

    ShaderCompileResult compile_slang_spirv(rust::Str source, rust::Str module_name,
                                            rust::Slice<const EntryPointRequest> entry_points);

} // namespace sturdy_rs::shader_compiler
