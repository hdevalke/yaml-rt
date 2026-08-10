use std::env;
use std::path::{Path, PathBuf};

const RAPIDYAML_SOURCES: &[&str] = &[
    "src/c4/yml/common.cpp",
    "src/c4/yml/emit_buf.cpp",
    "src/c4/yml/emit_file.cpp",
    "src/c4/yml/node_type.cpp",
    "src/c4/yml/parse.cpp",
    "src/c4/yml/reference_resolver.cpp",
    "src/c4/yml/scalar_style.cpp",
    "src/c4/yml/tag.cpp",
    "src/c4/yml/tree.cpp",
    "src/c4/yml/version.cpp",
];

const C4CORE_SOURCES: &[&str] = &[
    "ext/c4core.src/c4/base64.cpp",
    "ext/c4core.src/c4/error.cpp",
    "ext/c4core.src/c4/format.cpp",
    "ext/c4core.src/c4/language.cpp",
    "ext/c4core.src/c4/memory_util.cpp",
    "ext/c4core.src/c4/utf.cpp",
    "ext/c4core.src/c4/version.cpp",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/rapidyaml_shim.cpp");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RAPIDYAML_BASELINE");

    if env::var_os("CARGO_FEATURE_RAPIDYAML_BASELINE").is_none() {
        return;
    }

    let rapidyaml = workspace_root().join("third_party/rapidyaml");
    assert!(
        rapidyaml.join("CMakeLists.txt").is_file(),
        "Rapid YAML sources are missing; run \
             `git submodule update --init third_party/rapidyaml`"
    );

    println!("cargo:rerun-if-changed={}", rapidyaml.display());

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++11")
        .warnings(false)
        .define("RYML_DEFAULT_CALLBACK_USES_EXCEPTIONS", None)
        .include(rapidyaml.join("src"))
        .include(rapidyaml.join("ext/c4core.src"))
        .file("native/rapidyaml_shim.cpp");

    if env::var("DEBUG").as_deref() == Ok("false") {
        build.define("NDEBUG", None);
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("/EHsc");
    }

    add_sources(&mut build, &rapidyaml, RAPIDYAML_SOURCES);
    add_sources(&mut build, &rapidyaml, C4CORE_SOURCES);
    build.compile("yaml_rt_rapidyaml");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is set"))
        .join("../..")
}

fn add_sources(build: &mut cc::Build, root: &Path, sources: &[&str]) {
    for source in sources {
        let path = root.join(source);
        println!("cargo:rerun-if-changed={}", path.display());
        build.file(path);
    }
}
