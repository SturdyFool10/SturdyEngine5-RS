//! Compiles Slang shader source to raw SPIR-V bytes for [`crate::rhi::Rhi::create_shader_module`].
//!
//! Distinct from [`crate::assets::Assets::load_shader`], which loads a `.slang` **file** into an
//! engine-managed [`crate::assets::Asset`] for the higher-level renderer/material system — there
//! is no bridge from that `Asset` back to a raw [`crate::rhi::ShaderModuleHandle`]. Use this
//! module when you need actual bytecode to hand to the low-level RHI pipeline-creation path
//! directly (e.g. a custom render pipeline outside the engine's own material system, or a test).

use sturdy_sys::shader_compiler::ffi;

/// Which shader stage a requested entry point targets. Mirrors the subset of `SFT::Core::Slang::
/// ShaderStage` this binding exposes; see [`sturdy_sys::shader_compiler::ffi::ShaderStage`] if a
/// future caller needs a stage not listed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

impl From<ShaderStage> for ffi::ShaderStage {
    fn from(stage: ShaderStage) -> Self {
        match stage {
            ShaderStage::Vertex => ffi::ShaderStage::Vertex,
            ShaderStage::Fragment => ffi::ShaderStage::Fragment,
            ShaderStage::Compute => ffi::ShaderStage::Compute,
        }
    }
}

/// One entry point to compile, e.g. `EntryPoint::new("vertexMain", ShaderStage::Vertex)`.
#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub name: String,
    pub stage: ShaderStage,
}

impl EntryPoint {
    pub fn new(name: impl Into<String>, stage: ShaderStage) -> Self {
        Self { name: name.into(), stage }
    }
}

/// A Slang compile error: the compiler's own message, plus source diagnostics when available
/// (folded together into one string by the C++ side — see `sturdy-sys`'s `describe` helper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError(pub String);

impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CompileError {}

/// Compiles `source` (a self-contained Slang module — no `import`s; this binding passes no search
/// path for the compiler to resolve them against) targeting SPIR-V, returning each requested
/// entry point's bytecode in the same order as `entry_points`. `module_name` only affects
/// diagnostic messages and Slang's internal module cache key.
pub fn compile_spirv(
    source: &str,
    module_name: &str,
    entry_points: &[EntryPoint],
) -> Result<Vec<Vec<u8>>, CompileError> {
    let requests: Vec<ffi::EntryPointRequest> = entry_points
        .iter()
        .map(|e| ffi::EntryPointRequest { name: e.name.clone(), stage: e.stage.into() })
        .collect();
    let result = ffi::compile_slang_spirv(source, module_name, &requests);
    if !result.ok {
        return Err(CompileError(result.error));
    }
    Ok(result.entry_points.into_iter().map(|e| e.bytes).collect())
}
