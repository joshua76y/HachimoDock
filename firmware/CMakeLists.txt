# [Input] ESP-IDF environment, project sources, optional target-specific version override, and Git build identity.
# [Output] Versioned ESP32-P4 firmware project with target-isolated protocol/build metadata.
# [Pos] Firmware build entry for firmware.
# [Sync] If this file changes, update `firmware/.folder.md` and README.md.

cmake_minimum_required(VERSION 3.16)
# Export the real ESP-IDF compilation context for SDK object checks.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

if("$ENV{PET_P4_CHIP_FAMILY}" STREQUAL "v3")
  set(SDKCONFIG_DEFAULTS "sdkconfig.defaults;sdkconfig.v3.defaults")
else()
  # Do not inherit an SDK-dependent default here: pre-v3 P4 silicon is not
  # ABI-compatible with rev 3.x, and the bounded-image post-action requires
  # the generated sdkconfig to state the legacy 1..199 range explicitly.
  set(SDKCONFIG_DEFAULTS "sdkconfig.defaults;sdkconfig.v1.defaults")
endif()
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
if("$ENV{PET_P4_CHIP_FAMILY}" STREQUAL "v3")
  idf_build_set_property(DEPENDENCIES_LOCK "${CMAKE_CURRENT_LIST_DIR}/dependencies.v3.lock")
endif()
set(PROJECT_VER "0.7.105-p4-r185")
if(DEFINED ENV{PET_P4_PROJECT_VER_OVERRIDE} AND NOT "$ENV{PET_P4_PROJECT_VER_OVERRIDE}" STREQUAL "")
  set(PROJECT_VER "$ENV{PET_P4_PROJECT_VER_OVERRIDE}")
endif()
set(PET_P4_UART_BAUD 4000000)
if(DEFINED ENV{PET_P4_UART_BAUD_OVERRIDE} AND NOT "$ENV{PET_P4_UART_BAUD_OVERRIDE}" STREQUAL "")
  set(PET_P4_UART_BAUD "$ENV{PET_P4_UART_BAUD_OVERRIDE}")
endif()
set(PET_P4_PROTOCOL_SCHEMA 7)
set(PET_P4_GIT_SHA "unknown")
set(PET_P4_BUILD_DIRTY 0)
find_program(PET_P4_GIT_EXECUTABLE git)
if(PET_P4_GIT_EXECUTABLE)
  execute_process(
    COMMAND ${PET_P4_GIT_EXECUTABLE} -C ${CMAKE_CURRENT_LIST_DIR} rev-parse --short=12 HEAD
    RESULT_VARIABLE PET_P4_GIT_SHA_RESULT
    OUTPUT_VARIABLE PET_P4_GIT_SHA_OUTPUT
    OUTPUT_STRIP_TRAILING_WHITESPACE
    ERROR_QUIET
  )
  if(PET_P4_GIT_SHA_RESULT EQUAL 0 AND NOT PET_P4_GIT_SHA_OUTPUT STREQUAL "")
    set(PET_P4_GIT_SHA "${PET_P4_GIT_SHA_OUTPUT}")
  endif()
  execute_process(
    COMMAND ${PET_P4_GIT_EXECUTABLE} -C ${CMAKE_CURRENT_LIST_DIR} status --porcelain --untracked-files=normal
    RESULT_VARIABLE PET_P4_GIT_STATUS_RESULT
    OUTPUT_VARIABLE PET_P4_GIT_STATUS_OUTPUT
    OUTPUT_STRIP_TRAILING_WHITESPACE
    ERROR_QUIET
  )
  if(PET_P4_GIT_STATUS_RESULT EQUAL 0 AND NOT PET_P4_GIT_STATUS_OUTPUT STREQUAL "")
    set(PET_P4_BUILD_DIRTY 1)
  endif()
endif()
set(PET_P4_BUILD_ID "${PROJECT_VER}+${PET_P4_GIT_SHA}")
if(PET_P4_BUILD_DIRTY)
  string(APPEND PET_P4_BUILD_ID "-dirty")
endif()
message(STATUS "P4 build identity: ${PET_P4_BUILD_ID}, protocol schema ${PET_P4_PROTOCOL_SCHEMA}, UART ${PET_P4_UART_BAUD}")
project(pet_manager_p4_runtime)

# ESP-Hosted 1.4.7 asserts when Wi-Fi TX or streaming SDIO RX runs out of DMA
# buffers. Generate checked copies with normal failure paths; do not edit the
# managed component cache. Changed upstream sources fail the build for review.
if(CONFIG_ESP_HOSTED_ENABLED)
  idf_component_get_property(hosted_lib espressif__esp_hosted COMPONENT_LIB)
  idf_component_get_property(hosted_dir espressif__esp_hosted COMPONENT_DIR)
  set(hosted_original "${hosted_dir}/host/drivers/transport/transport_drv.c")
  set(hosted_guarded "${CMAKE_BINARY_DIR}/pet_hosted_transport_guarded.c")
  set(hosted_patcher "${CMAKE_CURRENT_LIST_DIR}/tools/patch_hosted_transport.py")
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${hosted_original}" "${hosted_patcher}")
  execute_process(COMMAND "${PYTHON}" "${hosted_patcher}" "${hosted_original}" "${hosted_guarded}"
                  RESULT_VARIABLE hosted_patch_result)
  if(NOT hosted_patch_result EQUAL 0)
    message(FATAL_ERROR "Cannot safely apply ESP-Hosted TX allocation guard")
  endif()
  if(CONFIG_ESP_HOSTED_SDIO_HOST_INTERFACE)
    set(sdio_original "${hosted_dir}/host/drivers/transport/sdio/sdio_drv.c")
    set(sdio_guarded "${CMAKE_BINARY_DIR}/pet_hosted_sdio_guarded.c")
    set(sdio_patcher "${CMAKE_CURRENT_LIST_DIR}/tools/patch_hosted_sdio.py")
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${sdio_original}" "${sdio_patcher}")
    execute_process(COMMAND "${PYTHON}" "${sdio_patcher}" "${sdio_original}" "${sdio_guarded}"
                    RESULT_VARIABLE sdio_patch_result)
    if(NOT sdio_patch_result EQUAL 0)
      message(FATAL_ERROR "Cannot safely apply ESP-Hosted SDIO RX allocation guard")
    endif()
  endif()
  get_target_property(hosted_sources ${hosted_lib} SOURCES)
  set(hosted_new_sources "")
  set(hosted_replacements 0)
  set(sdio_replacements 0)
  foreach(hosted_source IN LISTS hosted_sources)
    if(hosted_source MATCHES "(^|/)host/drivers/transport/transport_drv\\.c$")
      list(APPEND hosted_new_sources "${hosted_guarded}")
      math(EXPR hosted_replacements "${hosted_replacements} + 1")
    elseif(CONFIG_ESP_HOSTED_SDIO_HOST_INTERFACE AND hosted_source MATCHES "(^|/)host/drivers/transport/sdio/sdio_drv\\.c$")
      list(APPEND hosted_new_sources "${sdio_guarded}")
      math(EXPR sdio_replacements "${sdio_replacements} + 1")
    else()
      list(APPEND hosted_new_sources "${hosted_source}")
    endif()
  endforeach()
  if(NOT hosted_replacements EQUAL 1)
    message(FATAL_ERROR "Expected exactly one ESP-Hosted transport source, found ${hosted_replacements}")
  endif()
  if(CONFIG_ESP_HOSTED_SDIO_HOST_INTERFACE AND NOT sdio_replacements EQUAL 1)
    message(FATAL_ERROR "Expected exactly one ESP-Hosted SDIO source, found ${sdio_replacements}")
  endif()
  set_property(TARGET ${hosted_lib} PROPERTY SOURCES "${hosted_new_sources}")
endif()
