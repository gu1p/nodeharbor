cmake_minimum_required(VERSION 3.22)
project(nodeharbor_android C)
if(NOT ANDROID_ABI STREQUAL "arm64-v8a")
  message(FATAL_ERROR "NodeHarbor currently supports ARM64 Android phones")
endif()
get_filename_component(REPOSITORY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../.." ABSOLUTE)
get_filename_component(NDK_BIN "${CMAKE_C_COMPILER}" DIRECTORY)
find_program(CARGO cargo REQUIRED)
set(RUST_OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/rust/aarch64-linux-android/release/libnodeharbor_core.a")
file(GLOB_RECURSE RUST_SOURCES CONFIGURE_DEPENDS "${REPOSITORY}/crates/core/src/*.rs")
add_custom_command(OUTPUT "${RUST_OUTPUT}"
  COMMAND "${CMAKE_COMMAND}" -E env
    "CARGO_TARGET_DIR=${CMAKE_CURRENT_BINARY_DIR}/rust"
    "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=${NDK_BIN}/aarch64-linux-android33-clang"
    "${CARGO}" build --manifest-path "${REPOSITORY}/crates/core/Cargo.toml" --locked --release --target aarch64-linux-android
  DEPENDS ${RUST_SOURCES} "${REPOSITORY}/Cargo.lock" "${REPOSITORY}/crates/core/Cargo.toml"
  WORKING_DIRECTORY "${REPOSITORY}" VERBATIM)
add_custom_target(android_owner_rules DEPENDS "${RUST_OUTPUT}")
add_library(owner_rules STATIC IMPORTED)
set_target_properties(owner_rules PROPERTIES IMPORTED_LOCATION "${RUST_OUTPUT}")
add_library(nodeharbor SHARED rules.c runtime.c)
add_dependencies(nodeharbor android_owner_rules)
target_compile_options(nodeharbor PRIVATE -Wall -Wextra -Werror -fvisibility=hidden)
target_link_options(nodeharbor PRIVATE -Wl,-z,max-page-size=16384 -Wl,--exclude-libs,ALL)
target_link_libraries(nodeharbor owner_rules log android dl m)
