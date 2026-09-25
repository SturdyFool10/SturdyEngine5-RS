//! Builds the vendored SturdyEngine 5 C++ engine as static archives and links them into the Rust
//! artifact, so the final executable needs nothing on the target machine but a GPU driver.
//!
//! Pipeline:
//!   1. `cxx-gen` turns the `#[cxx::bridge]` in `src/lib.rs` into `bridge.h` / `bridge.cc`.
//!   2. CMake configures the *unmodified* engine with `cmake/hook.cmake` injected, which adds the
//!      shim (+ generated bridge) as a target inside the engine's own build, so it inherits the
//!      engine's exact compiler flags, definitions and include paths.
//!   3. Ninja prints the authoritative link line of a probe executable; we mirror its archives (in
//!      order) and system libraries as Cargo link directives.
//!
//! Every `src/*.rs` file containing a `#[cxx::bridge]` module is its own bridge (see
//! `bridge_modules`); every `cpp/sturdy_rs/*.cpp` file is compiled into the shim automatically, so
//! adding a subsystem is "add the two files", not "edit build.rs".
//!
//! Environment knobs:
//!   STURDY_RS_TARGET_CPU   `-march` value for the C++ side (default: `x86-64-v2` on x86_64, never
//!                          `native`, which would make the binary crash on other machines)
//!   STURDY_RS_CMAKE_ARGS   extra whitespace-separated arguments for the CMake configure step
//!   STURDY_RS_BUILD_TYPE   CMake build type for the engine (default: Release)
//!
//! Cargo feature flags reach the engine's own `STURDY_*` CMake options (see `feature_cmake_args`
//! and `sturdy-sys/Cargo.toml`'s `[features]` doc comments) — this is how a downstream Rust
//! application chooses engine build configuration without ever touching CMake itself.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prevents system shared libraries from leaking into the otherwise fully static link.
const HERMETIC_CMAKE_ARGS: &[&str] = &[
    "-DCMAKE_DISABLE_FIND_PACKAGE_ZLIB=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_JPEG=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_JBIG=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_LibLZMA=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_ZSTD=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_WebP=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_LERC=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_libdeflate=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_Imath=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_openjph=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_liblzma=ON",
    "-DCMAKE_DISABLE_FIND_PACKAGE_libjpeg-turbo=ON",
    // libtiff decides its optional codecs from what it finds; pin them off so the result does
    // not depend on the build machine (the codecs left are the ones that need no external lib).
    "-Djpeg=OFF",
    "-Djpeg12=OFF",
    "-Dold-jpeg=OFF",
    "-Djbig=OFF",
    "-Dlzma=OFF",
    "-Dzstd=OFF",
    "-Dwebp=OFF",
    "-Dlerc=OFF",
    "-DOPENEXR_FORCE_INTERNAL_IMATH=ON",
    "-DOPENEXR_FORCE_INTERNAL_OPENJPH=ON",
];

/// System shared libraries the final binary is allowed to depend on: they are part of the OS
/// graphics stack (like the GPU driver itself) and cannot be bundled.
const ALLOWED_SYSTEM_LIBS: &[&str] = &["wayland-client"];
/// Linked by the engine's CMake but unnecessary: loaded at runtime through volk instead.
const DROPPED_SYSTEM_LIBS: &[&str] = &["vulkan"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let engine_src = manifest.join("vendor/sturdyengine5");

    for path in ["build.rs", "src", "cpp", "cmake", "vendor/sturdyengine5/CMakeLists.txt"] {
        println!("cargo:rerun-if-changed={path}");
    }
    for dir in [
        "Foundation", "Reflection", "Async", "Ecs", "Core", "RHI", "Renderer", "WindowManager",
        "Engine", "ApplicationHost", "Runtime", "cmake", "Shaders", "Fonts",
    ] {
        println!("cargo:rerun-if-changed=vendor/sturdyengine5/{dir}");
    }
    for var in ["STURDY_RS_TARGET_CPU", "STURDY_RS_CMAKE_ARGS", "STURDY_RS_BUILD_TYPE"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    assert!(
        engine_src.join("CMakeLists.txt").exists(),
        "SturdyEngine5 source missing at {}. It is not part of this repository: copy, symlink, or \
         submodule a checkout of the engine there (it must contain CMakeLists.txt at that path).",
        engine_src.display()
    );

    let bridge_cc = generate_bridges(&manifest, &out);
    let build_dir = out.join("engine-build");
    configure(&manifest, &out, &engine_src, &build_dir, &out.join("gen"), &bridge_cc);
    run(Command::new("cmake").args([
        "--build".as_ref(),
        build_dir.as_os_str(),
        "--target".as_ref(),
        "sturdy_rs_shim".as_ref(),
        "sturdy_rs_link_probe".as_ref(),
    ]));
    emit_link_directives(&build_dir);
}

/// Runs `cxx-gen` over every `src/*.rs` file that contains a `#[cxx::bridge]` module — one engine
/// subsystem per file (`src/lib.rs` -> `sturdy_rs/bridge.h`, `src/ecs.rs` -> `sturdy_rs/ecs_bridge.h`,
/// ...) — and returns the generated `.cc` paths for the shim to compile.
///
/// Adding a subsystem is adding a file here, in `cpp/sturdy_rs/`, and one `mod` line in
/// `src/lib.rs`; nothing else in this build script changes.
fn generate_bridges(manifest: &Path, out: &Path) -> Vec<PathBuf> {
    let gen_dir = out.join("gen");
    fs::create_dir_all(gen_dir.join("rust")).unwrap();
    fs::create_dir_all(gen_dir.join("sturdy_rs")).unwrap();
    fs::write(gen_dir.join("rust/cxx.h"), cxx_gen::HEADER).unwrap();

    let src_dir = manifest.join("src");
    let mut generated = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&src_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", src_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    entries.sort();

    for path in entries {
        let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if !source.contains("#[cxx::bridge") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_str().unwrap();
        // src/lib.rs keeps the original "bridge.h"/"bridge.cc" names the existing shim includes.
        let base = if stem == "lib" { "bridge".to_owned() } else { format!("{stem}_bridge") };

        let tokens: proc_macro2::TokenStream =
            source.parse().unwrap_or_else(|e| panic!("{} is not valid Rust tokens: {e}", path.display()));
        let code = cxx_gen::generate_header_and_cc(tokens, &cxx_gen::Opt::default())
            .unwrap_or_else(|e| panic!("cxx-gen failed on the bridge in {}: {e}", path.display()));

        let header = gen_dir.join(format!("sturdy_rs/{base}.h"));
        let cc = gen_dir.join(format!("sturdy_rs/{base}.cc"));
        fs::write(&header, &code.header).unwrap();
        fs::write(&cc, &code.implementation).unwrap();
        generated.push(cc);
    }
    generated
}

/// Every `.cpp` file under `cpp/sturdy_rs/` (the hand-written shim, one subsystem per file) plus
/// every generated bridge `.cc` (one per `src/*.rs` bridge module).
fn shim_sources(manifest: &Path, bridge_cc: &[PathBuf]) -> Vec<PathBuf> {
    let cpp_dir = manifest.join("cpp/sturdy_rs");
    let mut sources: Vec<_> = fs::read_dir(&cpp_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", cpp_dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "cpp"))
        .collect();
    sources.sort();
    sources.extend(bridge_cc.iter().cloned());
    sources
}

/// Cargo feature -> the `STURDY_*` CMake option it forwards to. Kept in one place so the mapping
/// in `sturdy-sys/Cargo.toml`'s `[features]` doc comments has a single source of truth to match.
fn feature_cmake_args() -> Vec<&'static str> {
    let mut args = Vec::new();
    if env::var_os("CARGO_FEATURE_WEBGPU").is_some() {
        args.push("-DSTURDY_ENABLE_WEBGPU=ON");
    }
    if env::var_os("CARGO_FEATURE_GLFW").is_some() {
        args.push("-DSTURDY_BUILD_GLFW_WINDOW_PROVIDER=ON");
    }
    if env::var_os("CARGO_FEATURE_TRACY").is_some() {
        args.push("-DSTURDY_ENABLE_TRACY=ON");
    }
    if env::var_os("CARGO_FEATURE_DEBUG_SECTIONS").is_some() {
        args.push("-DSTURDY_ENABLE_STATIC_DEAD_STRIPPING=OFF");
    }
    args
}

fn configure(
    manifest: &Path,
    out: &Path,
    engine_src: &Path,
    build_dir: &Path,
    gen_dir: &Path,
    bridge_cc: &[PathBuf],
) {
    let cpu = env::var("STURDY_RS_TARGET_CPU").ok().unwrap_or_else(|| {
        match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "x86-64-v2".into(),
            _ => String::new(),
        }
    });
    // `native-cpu` opts out of the portable baseline; STURDY_RS_TARGET_CPU still wins if both are
    // set, since it's the more specific ask.
    let cpu = if env::var_os("CARGO_FEATURE_NATIVE_CPU").is_some() && env::var_os("STURDY_RS_TARGET_CPU").is_none() {
        "native".to_owned()
    } else {
        cpu
    };
    let build_type = env::var("STURDY_RS_BUILD_TYPE").unwrap_or_else(|_| "Release".into());
    // Deliberately explicit: a builder's CFLAGS/CXXFLAGS (commonly `-march=native`) must not leak
    // into an artifact meant to run on other machines.
    // `-mxsave` on x86: Foundation's CPUID probe reads XCR0 via `xgetbv` (after checking OSXSAVE),
    // which clang only accepts with that feature enabled; `-march=native` used to imply it.
    let mut cxx_flags = if cpu.is_empty() { String::new() } else { format!("-march={cpu}") };
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        cxx_flags.push_str(" -mxsave");
    }

    let shim_sources = shim_sources(manifest, bridge_cc)
        .into_iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(";");
    let include_dirs = format!("{};{}", manifest.join("cpp").display(), gen_dir.display());

    let mut cmd = Command::new("cmake");
    cmd.arg("-S").arg(engine_src).arg("-B").arg(build_dir).args(["-G", "Ninja"]);
    cmd.arg(format!("-DCMAKE_BUILD_TYPE={build_type}"))
        .arg("-DCMAKE_C_COMPILER=clang")
        .arg("-DCMAKE_CXX_COMPILER=clang++")
        .arg(format!("-DCMAKE_C_FLAGS={cxx_flags}"))
        .arg(format!("-DCMAKE_CXX_FLAGS={cxx_flags}"))
        .arg("-DCMAKE_POSITION_INDEPENDENT_CODE=ON")
        // Rust's linker cannot consume clang's ThinLTO bitcode unless its LLVM matches exactly.
        .arg("-DSTURDY_ENABLE_LTO=OFF")
        .arg("-DSTURDY_BUILD_SHARED_LIBS=OFF")
        .arg("-DSTURDY_BUILD_RUNTIME_DEMO=OFF")
        .arg("-DSTURDY_USE_COMPILER_LAUNCHER=OFF")
        // Build every dependency from source: a system copy would defeat the static bundle (and
        // stray CMake packages, e.g. CUDA's harfbuzz config, break the configure outright).
        .arg("-DFETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER")
        // Third-party projects (libtiff, OpenEXR, ...) call find_package on their own and would
        // otherwise bind to whatever shared libraries the build machine has installed. Turn those
        // lookups off so each codec degrades cleanly or falls back to a vendored source build.
        .args(HERMETIC_CMAKE_ARGS)
        // Cargo feature flags -> engine CMake options; see `feature_cmake_args`.
        .args(feature_cmake_args())
        // Keep the vendored source tree pristine: the dependency cache lives in OUT_DIR.
        .arg(format!("-DSTURDY_DEPS_CACHE_DIR={}", out.join("deps").display()))
        .arg(format!("-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES={}", manifest.join("cmake/hook.cmake").display()))
        .arg(format!("-DSTURDY_RS_SHIM_CMAKE={}", manifest.join("cmake/shim.cmake").display()))
        .arg(format!("-DSTURDY_RS_SHIM_SOURCES={shim_sources}"))
        .arg(format!("-DSTURDY_RS_SHIM_INCLUDE_DIRS={include_dirs}"));
    if let Ok(extra) = env::var("STURDY_RS_CMAKE_ARGS") {
        cmd.args(extra.split_whitespace());
    }
    // Configure-time compiler flags come only from the -D arguments above.
    cmd.env_remove("CFLAGS").env_remove("CXXFLAGS").env_remove("LDFLAGS");
    run(&mut cmd);
}

fn run(cmd: &mut Command) {
    cmd.env_remove("CFLAGS").env_remove("CXXFLAGS").env_remove("LDFLAGS");
    let status = cmd.status().unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed with {status}");
}

/// Splits a link command on whitespace, expanding `@response-file` arguments.
fn tokens(command: &str, build_dir: &Path) -> Vec<String> {
    let mut result = Vec::new();
    for token in command.split_whitespace() {
        match token.strip_prefix('@') {
            Some(rsp) if build_dir.join(rsp).is_file() => {
                let body = fs::read_to_string(build_dir.join(rsp)).unwrap();
                result.extend(body.split_whitespace().map(str::to_owned));
            }
            _ => result.push(token.to_owned()),
        }
    }
    result
}

fn emit_link_directives(build_dir: &Path) {
    let output = Command::new("ninja")
        .args(["-C"])
        .arg(build_dir)
        .args(["-t", "commands", "sturdy_rs_link_probe"])
        .output()
        .expect("failed to run `ninja -t commands`");
    assert!(output.status.success(), "ninja -t commands failed");
    let commands = String::from_utf8_lossy(&output.stdout);
    let link_line = commands
        .lines()
        .rfind(|l| l.contains("sturdy_rs_link_probe") && l.contains("-o "))
        .expect("could not find the probe link command");

    let mut seen_names: HashMap<String, PathBuf> = HashMap::new();
    let mut searched = Vec::new();
    let mut system_libs = Vec::new();

    for token in tokens(link_line, build_dir) {
        if let Some(lib) = token.strip_suffix(".a") {
            let path = build_dir.join(&token);
            let file = Path::new(lib).file_name().unwrap().to_string_lossy().into_owned();
            let name = file.strip_prefix("lib").unwrap_or(&file).to_owned();
            if name == "sturdy_rs_link_probe" {
                continue;
            }
            let dir = path.parent().unwrap().to_path_buf();
            if let Some(previous) = seen_names.insert(name.clone(), dir.clone()) {
                assert_eq!(
                    previous, dir,
                    "two different static archives named lib{name}.a; Cargo link-lib names cannot disambiguate them"
                );
                continue;
            }
            if !searched.contains(&dir) {
                println!("cargo:rustc-link-search=native={}", dir.display());
                searched.push(dir);
            }
            println!("cargo:rustc-link-lib=static={name}");
        } else if let Some(name) = shared_library_name(&token) {
            if DROPPED_SYSTEM_LIBS.contains(&name.as_str()) {
                continue;
            }
            assert!(
                ALLOWED_SYSTEM_LIBS.contains(&name.as_str()),
                "the engine link line pulls in system shared library `{token}`, which would make the \
                 executable depend on the build machine. Disable its find_package in \
                 HERMETIC_CMAKE_ARGS (build.rs) or add it to ALLOWED_SYSTEM_LIBS if it is part of the OS."
            );
            if !system_libs.contains(&name) {
                system_libs.push(name);
            }
        } else if let Some(lib) = token.strip_prefix("-l")
            && !system_libs.contains(&lib.to_owned())
        {
            system_libs.push(lib.to_owned());
        }
    }

    // Threading/dl are always needed by SDL3/volk/spdlog even when the link line omits them.
    for lib in ["pthread", "dl", "m"] {
        if !system_libs.iter().any(|l| l == lib) {
            system_libs.push(lib.into());
        }
    }
    for lib in system_libs {
        if lib == "stdc++" {
            continue; // handled below
        }
        println!("cargo:rustc-link-lib={lib}");
    }

    // The engine is C++26 and is built against a very recent libstdc++; a target machine's copy is
    // likely older, so the C++ runtime is bundled statically whenever the static archive exists.
    let static_stdcxx = Command::new("clang++")
        .arg("-print-file-name=libstdc++.a")
        .output()
        .ok()
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| p.is_absolute() && p.exists());
    if let Some(archive) = static_stdcxx {
        println!("cargo:rustc-link-search=native={}", archive.parent().unwrap().display());
        println!("cargo:rustc-link-lib=static=stdc++");
    } else {
        println!("cargo:warning=libstdc++.a not found; the binary will need a compatible libstdc++.so at runtime");
        println!("cargo:rustc-link-lib=stdc++");
    }
}

/// `/usr/lib/libfoo-bar.so.1.2` -> `foo-bar`.
fn shared_library_name(token: &str) -> Option<String> {
    let file = Path::new(token).file_name()?.to_str()?;
    let stem = file.strip_prefix("lib")?;
    let end = stem.find(".so")?;
    Some(stem[..end].to_owned())
}
