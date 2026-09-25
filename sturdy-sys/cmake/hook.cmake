# Injected with -DCMAKE_PROJECT_TOP_LEVEL_INCLUDES so the vendored engine tree stays unmodified.
# Deferred to the end of the top-level directory so every Sturdy::* target already exists when the
# shim links against them (the engine registers its packages by directory glob).
cmake_language(DEFER DIRECTORY "${CMAKE_SOURCE_DIR}" CALL include "${STURDY_RS_SHIM_CMAKE}")
