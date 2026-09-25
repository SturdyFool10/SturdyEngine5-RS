// Rust <-> SturdyEngine 5 reflection shim: introspection (and limited live-instance access) over
// the process-wide `SFT::Reflection::TypeRegistry`. See reflection.rs's module doc comment for
// scope.
#pragma once

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include <rust/cxx.h>

#include <Reflection/TypeInfoBuilder.hpp>

namespace sturdy_rs::reflection {

    struct TypeSummary;
    struct FieldSummary;
    struct EnumSummary;
    struct EnumeratorSummary;
    struct MethodSummary;
    struct ConstructorSummary;
    struct MethodInvokeResult;
    struct ContainerSummary;
    struct AttributeSummary;
    struct EventSummary;
    struct CallableLayout;
    struct RustReflectionCallback;
    enum class OverlayTarget : std::uint8_t;

    /// See `DynamicTypeBuilder` in reflection.rs. Methods are held back until `finish` so their
    /// invoke trampoline slots are only claimed by types that actually get registered.
    struct DynamicTypeBuilder {
        struct PendingMethod {
            std::string name;
            SFT::Reflection::TypeId return_type;
            std::vector<SFT::Reflection::TypeId> param_types;
            RustReflectionCallback *callback = nullptr;
        };

        DynamicTypeBuilder(std::string_view name, std::size_t size, std::size_t align);
        ~DynamicTypeBuilder();
        DynamicTypeBuilder(const DynamicTypeBuilder &) = delete;
        DynamicTypeBuilder &operator=(const DynamicTypeBuilder &) = delete;

        std::string name;
        std::size_t size = 0;
        std::vector<std::uint8_t> default_bytes;
        SFT::Reflection::TypeInfoBuilder builder;
        std::vector<PendingMethod> methods;
        rust::String error;
    };

    rust::Vec<rust::String> reflection_type_names();

    bool reflection_find_type(rust::Str name, TypeSummary &out);
    rust::Vec<FieldSummary> reflection_type_fields(rust::Str name);

    bool reflection_find_enum(rust::Str name, EnumSummary &out);
    rust::Vec<EnumeratorSummary> reflection_enum_values(rust::Str name);

    rust::Vec<MethodSummary> reflection_type_methods(rust::Str name);
    rust::Vec<ConstructorSummary> reflection_type_constructors(rust::Str name);

    bool reflection_default_construct(rust::Str name, rust::Slice<std::uint8_t> out);
    bool reflection_destroy_instance(rust::Str name, rust::Slice<std::uint8_t> instance);

    MethodInvokeResult reflection_invoke_method0(rust::Str type_name, rust::Str method_name, rust::Slice<std::uint8_t> object);

    bool reflection_field_container(rust::Str type_name, rust::Str field_name, ContainerSummary &out);

    bool reflection_get_static_field(rust::Str type_name, rust::Str field_name, rust::Slice<std::uint8_t> out);
    bool reflection_set_static_field(rust::Str type_name, rust::Str field_name, rust::Slice<const std::uint8_t> data);

    std::uint64_t reflection_container_size(rust::Str type_name, rust::Str field_name, rust::Slice<const std::uint8_t> instance);
    bool reflection_container_get_element(rust::Str type_name, rust::Str field_name, rust::Slice<const std::uint8_t> instance,
                                          std::uint64_t index, rust::Slice<std::uint8_t> out);
    bool reflection_container_set_element(rust::Str type_name, rust::Str field_name, rust::Slice<std::uint8_t> instance,
                                          std::uint64_t index, rust::Slice<const std::uint8_t> data);
    bool reflection_container_resize(rust::Str type_name, rust::Str field_name, rust::Slice<std::uint8_t> instance,
                                     std::uint64_t new_size);

    rust::Vec<AttributeSummary> reflection_type_attributes(rust::Str name);
    rust::Vec<AttributeSummary> reflection_field_attributes(rust::Str type_name, rust::Str field_name);

    rust::Vec<EventSummary> reflection_type_events(rust::Str name);
    bool reflection_find_event(rust::Str type_name, rust::Str event_name, EventSummary &out);
    bool reflection_fire_event(rust::Str type_name, rust::Str event_name, rust::Slice<std::uint8_t> object,
                               rust::Slice<const std::uint32_t> arg_sizes, rust::Slice<const std::uint8_t> args_data);

    void reflection_register_demo_type();

    CallableLayout reflection_method_layout(rust::Str type_name, rust::Str method_name);
    CallableLayout reflection_event_layout(rust::Str type_name, rust::Str event_name);
    bool reflection_set_method_override(rust::Str type_name, rust::Str method_name, RustReflectionCallback *callback);
    bool reflection_clear_method_override(rust::Str type_name, rust::Str method_name);
    std::uint64_t reflection_add_method_hook(rust::Str type_name, rust::Str method_name, bool after,
                                             RustReflectionCallback *callback);
    bool reflection_remove_method_hook(rust::Str type_name, rust::Str method_name, bool after, std::uint64_t subscription);
    std::uint64_t reflection_subscribe_event(rust::Str type_name, rust::Str event_name, RustReflectionCallback *callback);
    bool reflection_unsubscribe_event(rust::Str type_name, rust::Str event_name, std::uint64_t subscription);

    std::uint64_t reflection_type_size(rust::Str type_name);
    std::unique_ptr<DynamicTypeBuilder> reflection_type_builder_new(rust::Str name, std::uint64_t size, std::uint64_t align,
                                                                    rust::Slice<const std::uint8_t> default_bytes);
    rust::String reflection_type_builder_add_field(DynamicTypeBuilder &builder, rust::Str name, std::uint64_t offset,
                                                   rust::Str type_name, const rust::Vec<AttributeSummary> &attributes);
    rust::String reflection_type_builder_add_method(DynamicTypeBuilder &builder, rust::Str name, rust::Str return_type_name,
                                                    const rust::Vec<rust::String> &param_type_names,
                                                    RustReflectionCallback *callback);
    rust::String reflection_type_builder_add_event(DynamicTypeBuilder &builder, rust::Str name,
                                                   const rust::Vec<rust::String> &param_type_names);
    void reflection_type_builder_add_type_attribute(DynamicTypeBuilder &builder, const AttributeSummary &attribute);
    rust::String reflection_type_builder_finish(std::unique_ptr<DynamicTypeBuilder> builder);
    bool reflection_unregister_type(rust::Str type_name);
    rust::String reflection_invoke_method(rust::Str type_name, rust::Str method_name, rust::Slice<std::uint8_t> object,
                                          rust::Slice<const std::uint32_t> arg_sizes, rust::Slice<const std::uint8_t> args_data,
                                          rust::Slice<std::uint8_t> out_return);

    bool reflection_overlay_set_name(OverlayTarget target, rust::Str type_name, rust::Str member, rust::Str name);
    bool reflection_overlay_clear_name(OverlayTarget target, rust::Str type_name, rust::Str member);
    bool reflection_overlay_add_attribute(OverlayTarget target, rust::Str type_name, rust::Str member,
                                          const AttributeSummary &attribute);
    bool reflection_overlay_clear(OverlayTarget target, rust::Str type_name, rust::Str member);
    bool reflection_overlay_effective_name(OverlayTarget target, rust::Str type_name, rust::Str member, rust::String &out);
    rust::Vec<AttributeSummary> reflection_overlay_effective_attributes(OverlayTarget target, rust::Str type_name, rust::Str member);

} // namespace sturdy_rs::reflection
