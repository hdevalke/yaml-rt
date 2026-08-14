cmake_minimum_required(VERSION 3.13)
project(yaml_rt_rapidyaml_benchmark LANGUAGES CXX)

find_program(CARGO_EXECUTABLE cargo REQUIRED)

get_filename_component(YAML_RT_WORKSPACE_ROOT "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)
set(YAML_RT_CARGO_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/cargo-target")
set(YAML_RT_STATIC_LIBRARY
    "${YAML_RT_CARGO_TARGET_DIR}/release/${CMAKE_STATIC_LIBRARY_PREFIX}yaml_rt_bench${CMAKE_STATIC_LIBRARY_SUFFIX}")

file(GLOB_RECURSE YAML_RT_RUST_SOURCES CONFIGURE_DEPENDS
    "${YAML_RT_WORKSPACE_ROOT}/crates/yaml-rt-bench/src/*.rs"
    "${YAML_RT_WORKSPACE_ROOT}/crates/yaml-rt-core/src/*.rs")
set(YAML_RT_CARGO_INPUTS
    "${YAML_RT_WORKSPACE_ROOT}/Cargo.toml"
    "${YAML_RT_WORKSPACE_ROOT}/Cargo.lock"
    "${YAML_RT_WORKSPACE_ROOT}/crates/yaml-rt-bench/Cargo.toml"
    "${YAML_RT_WORKSPACE_ROOT}/crates/yaml-rt-bench/build.rs"
    "${YAML_RT_WORKSPACE_ROOT}/crates/yaml-rt-core/Cargo.toml"
    ${YAML_RT_RUST_SOURCES})
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS ${YAML_RT_CARGO_INPUTS})

# Rust static libraries require a target-specific set of native system
# libraries. Ask rustc for that set instead of duplicating platform knowledge
# in this adapter. The same invocation creates the initial release artifact.
execute_process(
    COMMAND "${CARGO_EXECUTABLE}" rustc
        --manifest-path "${YAML_RT_WORKSPACE_ROOT}/Cargo.toml"
        --package yaml-rt-bench
        --lib
        --release
        --target-dir "${YAML_RT_CARGO_TARGET_DIR}"
        --
        --print=native-static-libs
    WORKING_DIRECTORY "${YAML_RT_WORKSPACE_ROOT}"
    RESULT_VARIABLE YAML_RT_CARGO_RESULT
    OUTPUT_VARIABLE YAML_RT_CARGO_STDOUT
    ERROR_VARIABLE YAML_RT_CARGO_STDERR)
if(NOT YAML_RT_CARGO_RESULT EQUAL 0)
    message(FATAL_ERROR
        "failed to build the yaml-rt benchmark library:\n${YAML_RT_CARGO_STDOUT}\n${YAML_RT_CARGO_STDERR}")
endif()
string(REGEX MATCH "native-static-libs: ([^\r\n]+)" YAML_RT_NATIVE_MATCH
    "${YAML_RT_CARGO_STDOUT}\n${YAML_RT_CARGO_STDERR}")
if(NOT YAML_RT_NATIVE_MATCH)
    message(FATAL_ERROR
        "rustc did not report native static libraries:\n${YAML_RT_CARGO_STDOUT}\n${YAML_RT_CARGO_STDERR}")
endif()
set(YAML_RT_NATIVE_LIBRARIES "${CMAKE_MATCH_1}")
separate_arguments(YAML_RT_NATIVE_LIBRARIES NATIVE_COMMAND "${YAML_RT_NATIVE_LIBRARIES}")

add_custom_command(
    OUTPUT "${YAML_RT_STATIC_LIBRARY}"
    COMMAND "${CARGO_EXECUTABLE}" build
        --manifest-path "${YAML_RT_WORKSPACE_ROOT}/Cargo.toml"
        --package yaml-rt-bench
        --lib
        --release
        --target-dir "${YAML_RT_CARGO_TARGET_DIR}"
    WORKING_DIRECTORY "${YAML_RT_WORKSPACE_ROOT}"
    DEPENDS ${YAML_RT_CARGO_INPUTS}
    COMMENT "Building yaml-rt's release benchmark library"
    VERBATIM)
add_custom_target(yaml-rt-benchmark-library DEPENDS "${YAML_RT_STATIC_LIBRARY}")

set(RYML_BUILD_BENCHMARKS ON CACHE BOOL "" FORCE)
set(RYML_INSTALL OFF CACHE BOOL "" FORCE)
add_subdirectory("${YAML_RT_WORKSPACE_ROOT}/third_party/rapidyaml" rapidyaml)

if(NOT TARGET ryml-bm-parse)
    message(FATAL_ERROR "Rapid YAML did not create the ryml-bm-parse target")
endif()

# Replace only the parse translation unit with a parent-owned wrapper. The
# wrapper includes the original file and appends yaml-rt's registration, so the
# pinned submodule remains clean and all upstream benchmarks stay intact.
get_target_property(RYML_PARSE_SOURCES ryml-bm-parse SOURCES)
list(FILTER RYML_PARSE_SOURCES EXCLUDE REGEX "(^|[/\\\\])bm_parse\\.cpp$")
set_property(TARGET ryml-bm-parse PROPERTY SOURCES "${RYML_PARSE_SOURCES}")
target_sources(ryml-bm-parse PRIVATE "${CMAKE_CURRENT_LIST_DIR}/bm_parse_yamlrt.cpp")
target_link_libraries(ryml-bm-parse PRIVATE
    "${YAML_RT_STATIC_LIBRARY}"
    ${YAML_RT_NATIVE_LIBRARIES})
add_dependencies(ryml-bm-parse yaml-rt-benchmark-library)
