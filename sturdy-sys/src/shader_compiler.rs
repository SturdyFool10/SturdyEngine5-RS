//! Raw `cxx` bindings for the engine's Slang shader compiler (`SFT::Core::Slang::ShaderCompiler`),
//! independent of any `Engine`/`RhiDevice` — it's a standalone, on-demand compiler object with no
//! device dependency, only ever asked to target a specific `ShaderTargetFormat` (SPIR-V here).
//!
//! ## Why this exists, distinct from `sturdy::assets::Assets::load_shader`
//!
//! `Assets::load_shader` compiles a `.slang` **file** into an engine-managed `Asset` for the
//! higher-level renderer/material system — there is no bridge from that `Asset` back to a raw
//! `sturdy::rhi::ShaderModuleHandle`. Exercising the low-level RHI pipeline-creation path
//! (`Rhi::create_shader_module`/`create_render_pipeline`) from Rust needs actual SPIR-V bytes, and
//! nothing else in this crate produces them. This wraps `ShaderCompiler::compile` + `Shader::
//! entry_point_code` directly: source text in, one `Vec<u8>` of SPIR-V per requested entry point
//! out. It exists to make the RHI surface actually testable end to end (a real pipeline, a real
//! draw), not to duplicate the asset system's own shader-loading path.

#[cxx::bridge(namespace = "sturdy_rs::shader_compiler")]
pub mod ffi {
    /// Mirrors the subset of `SFT::Core::Slang::ShaderStage` a hand-written test/demo shader would
    /// plausibly use; add more variants here if a future caller needs them; the C++ side's `map`
    /// covers the engine's full enum regardless.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShaderStage {
        Vertex,
        Fragment,
        Compute,
    }

    #[derive(Debug, Clone)]
    struct EntryPointRequest {
        name: String,
        stage: ShaderStage,
    }

    /// One requested entry point's compiled SPIR-V, parallel to the `EntryPointRequest` slice
    /// passed to [`compile_slang_spirv`].
    #[derive(Debug, Clone, Default)]
    struct CompiledEntryPoint {
        bytes: Vec<u8>,
    }

    #[derive(Debug, Clone, Default)]
    struct ShaderCompileResult {
        ok: bool,
        /// Populated when `!ok`: `ShaderError::message`, plus `diagnostics` (the compiler's own
        /// error text, e.g. line/column-annotated syntax errors) when non-empty.
        error: String,
        /// Exactly `entry_points.len()` entries (the request slice), in the same order, when `ok`.
        entry_points: Vec<CompiledEntryPoint>,
    }

    unsafe extern "C++" {
        include!("sturdy_rs/shader_compiler.hpp");

        /// Compiles `source` (a self-contained Slang module — no `import`s, since this passes no
        /// search path for the compiler to resolve them against) targeting SPIR-V, and returns
        /// each requested entry point's bytecode in request order. `module_name` only affects
        /// diagnostic messages and Slang's internal module cache key.
        fn compile_slang_spirv(source: &str, module_name: &str, entry_points: &[EntryPointRequest]) -> ShaderCompileResult;
    }
}
