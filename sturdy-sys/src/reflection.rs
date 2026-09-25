//! Type introspection *and* limited live-instance access over the engine's process-wide reflection
//! registry (`SFT::Reflection::TypeRegistry`, see `Reflection/TypeRegistry.hpp`).
//!
//! Unlike everything in `lib.rs`/`ui.rs`, none of this takes an `EngineView`: the registry is a
//! singleton independent of any particular `Engine` instance (confirmed by `TypeRegistry::instance()`
//! and by `FFI/src/FFI/Reflection.cpp`, which never resolves an engine handle either).
//!
//! Scope, in the order this module built it out:
//!  1. Type/field/enum descriptors (the original pass): listing registered type names, a type's
//!     size/alignment/fields, and an enum's enumerators by name.
//!  2. Live instance field read/write: pure offset arithmetic on the Rust side against a `&[u8]`/
//!     `&mut [u8]` view the caller already has (see `sturdy::reflection::read_field`/`write_field`)
//!     -- `FieldSummary` already carried `offset`/`size`, so this needed no new C++ entry point at
//!     all.
//!  3. Constructors: `reflection_default_construct`/`reflection_destroy_instance`, wrapping
//!     `TypeInfo::default_construct`/`destroy`. Parameterized constructors (`ConstructorInfo`,
//!     found by signature rather than name) are enumerated as descriptors
//!     (`reflection_type_constructors`) but not invokable from here -- see module docs on
//!     `reflection_type_methods` for why arbitrary-argument invocation from a dynamic Rust call
//!     site is out of scope in general.
//!  4. Methods: `reflection_type_methods` enumerates every declared method's name/arity/parameter
//!     and return type names. `reflection_invoke_method0` invokes a method by name when it takes no
//!     arguments and returns either `void` or a type whose size/shape this binding can determine
//!     ahead of time (a registered `TypeInfo`, or one of the fixed-vocabulary fundamental names in
//!     `FundamentalTypeName`, `StaticTypeId.hpp`) -- see its doc comment. Arbitrary-signature
//!     invocation is a deliberate non-goal: `MethodInfo::invoke` takes an array of already-typed,
//!     caller-marshalled `const void*` argument pointers (see `Invoke.hpp`), and there is no generic
//!     way for a dynamic Rust caller to conjure a correctly-typed, correctly-laid-out C++ value for
//!     an arbitrary parameter type it only knows by name.
//!  5. Container/map/set/optional element introspection: `reflection_field_container` classifies a
//!     non-trivial field (`FieldSummary::primitive_kind == None`) as one of those shapes and reports
//!     its element (and, for maps, key) type name -- descriptor-only, same shape as the original
//!     field/enum work.
//!  6. Attributes: `reflection_type_attributes`/`reflection_field_attributes` walk
//!     `TypeInfo`/`FieldInfo::attributes` (`Attribute.hpp`'s `bool`/`i64`/`f64`/`UString` variant).
//!  7. Events: `reflection_type_events`/`reflection_find_event` enumerate/look up name + parameter
//!     type names (`EventInfo`/`Multicast.hpp`); `reflection_fire_event` fires one with
//!     caller-marshalled argument bytes (strictly size-checked against each parameter's own
//!     registered/fundamental size before ever touching the C++ call -- see its own doc comment).
//!     Subscribing a Rust closure is point 10.
//!  8. Static fields: `reflection_get_static_field`/`reflection_set_static_field`. Unlike instance
//!     fields (pure offset arithmetic against a caller-supplied `&[u8]`, see point 2), a static
//!     field's one true address (`FieldInfo::static_address`) lives inside the C++ process with no
//!     Rust-visible byte view to slice into, so this needed real `copy_static_field_out`/`_in`
//!     calls (`FieldInfo.hpp`) rather than reusing the instance-field code path.
//!  9. Container element access: `reflection_container_size`/`_get_element`/`_set_element`/
//!     `_resize`, layered on the same `field.container`/`ContainerInfo` (`ContainerInfo.hpp`) that
//!     point 5 already classifies -- restricted to `element_trivial` elements (mirrors point 2's own
//!     restriction to `Trivial` fields): a non-trivial element's `get_element` placement-copy would
//!     leave a live C++ object inside a plain Rust byte buffer with no way to safely destroy it.
//! 10. Overrides, hooks, and event subscription (`reflection_set_method_override`,
//!     `reflection_add_method_hook`, `reflection_subscribe_event`): Rust closures called back from
//!     C++ through `rust_reflection_callback_run`, with `guard()` abort-on-panic like every other
//!     C++-crossing callback here. The closures are intentionally leaked on removal: `Multicast`
//!     fires from an RCU snapshot, so a call can still be in flight with the old `user_data`
//!     after unsubscribe returns.
//! 11. Runtime type registration (`DynamicTypeBuilder` -> `TypeInfoBuilder` +
//!     `TypeRegistry::register_type`, `reflection_unregister_type`): types declared entirely from
//!     Rust. They're plain bytes (memcpy move/copy, no-op destroy, default = copied bytes), so
//!     field/parameter/return types are limited to fundamentals and other runtime types. Method
//!     bodies are Rust closures; since `MethodInfo::invoke` has no user-data slot, each gets its
//!     own trampoline from a fixed 1024-entry table (never recycled). `reflection_invoke_method`
//!     is the arbitrary-arity invoke point 4 ruled out in general, made safe by the same
//!     plain-bytes restriction.
//! 12. Runtime overlay (`reflection_overlay_*`): presentation-only name and attribute overrides
//!     on a type or one of its fields/methods, via the registry's generation-checked handles.
//!
//! One older asymmetry, unchanged since the original pass: there is no `reflection_enum_names`
//! alongside `reflection_type_names`. `TypeRegistry` exposes `for_each_type` for the struct/class
//! side but has no equivalent `for_each_enum` (its `enum_infos_` deque is private, see
//! `TypeRegistry.hpp`), so enumerating every registered enum's name process-wide is not something
//! the public C++ API supports today -- only looking one up by an already-known name
//! (`reflection_find_enum`) is.

#[cxx::bridge(namespace = "sturdy_rs::reflection")]
pub mod ffi {
    /// Mirrors `SFT::Reflection::PrimitiveKind` -- only meaningful on a field/enumerator/method
    /// return whose trivial-ness is otherwise confirmed; `None` otherwise (struct, container, map,
    /// set, optional, void, ...).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum PrimitiveKind {
        #[default]
        None,
        Bool,
        SignedInt,
        UnsignedInt,
        Float,
    }

    /// Mirrors the shape a non-trivial [`FieldSummary`] can have -- `SFT::Reflection::FieldInfo`'s
    /// mutually-exclusive `container`/`map`/`set`/`optional` pointers, collapsed into one tag.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum ContainerKind {
        #[default]
        None,
        /// `std::vector<T>`.
        Vector,
        /// `std::unordered_map<K, V>`.
        Map,
        /// `std::set<T>`/`std::unordered_set<T>`.
        Set,
        /// `std::optional<T>`/`std::unique_ptr<T>`/`std::shared_ptr<T>`.
        Optional,
    }

    /// Mirrors `SFT::Reflection::AttributeValue`'s alternatives (`std::variant<bool, i64, f64,
    /// UString>`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum AttributeKind {
        #[default]
        Bool,
        SignedInt,
        Float,
        String,
    }

    #[derive(Debug, Clone, Default)]
    struct TypeSummary {
        name: String,
        size: u64,
        align: u64,
        /// Number of instance fields (`TypeInfo::fields`; excludes `static_fields`).
        field_count: u64,
        /// Number of instance methods (`TypeInfo::methods`; excludes `static_methods`). See
        /// [`reflection_type_methods`] to enumerate them.
        method_count: u64,
        has_base: bool,
        /// Empty when `has_base` is `false`.
        base_name: String,
        /// Number of `TypeInfo::secondary_bases` entries.
        secondary_base_count: u64,
        /// Whether `TypeInfo::default_construct` is non-null (`T` is nothrow default-constructible)
        /// -- see [`reflection_default_construct`].
        has_default_constructor: bool,
    }

    #[derive(Debug, Clone, Default)]
    struct FieldSummary {
        name: String,
        /// The field's declared type's canonical name, or empty when that `TypeId` is not itself a
        /// registered type (e.g. some raw primitives are never separately reflected).
        field_type_name: String,
        offset: u64,
        size: u64,
        align: u64,
        is_static: bool,
        is_read_only: bool,
        is_trivial: bool,
        is_container: bool,
        is_map: bool,
        is_set: bool,
        is_optional: bool,
        primitive_kind: PrimitiveKind,
    }

    #[derive(Debug, Clone, Default)]
    struct EnumSummary {
        name: String,
        size: u64,
        align: u64,
        /// Canonical name of the enum's underlying integer type, if that type is itself
        /// registered (usually not, for a plain `i32`/`u8`/...); empty otherwise.
        underlying_type_name: String,
        enumerator_count: u64,
    }

    #[derive(Debug, Clone, Default)]
    struct EnumeratorSummary {
        name: String,
        /// Widest common integer representation (`EnumeratorInfo::value` is an `i64` in C++ too).
        value: i64,
    }

    /// One method or static method (`SFT::Reflection::MethodInfo`), minus the invocation machinery
    /// itself -- see [`reflection_invoke_method0`] for the (deliberately narrow) invocable subset.
    #[derive(Debug, Clone, Default)]
    struct MethodSummary {
        name: String,
        is_static: bool,
        /// Each parameter's canonical type name, in order; empty entries mean that parameter's type
        /// is not resolvable to a name this binding knows (see `field_type_name`'s doc comment for
        /// the same convention).
        param_type_names: Vec<String>,
        /// `"void"` for a `void`-returning method; otherwise the return type's canonical name, or
        /// empty when unresolvable.
        return_type_name: String,
        return_is_void: bool,
    }

    /// One parameterized constructor (`SFT::Reflection::ConstructorInfo`). Descriptor-only: see the
    /// module doc comment for why only the zero-argument case
    /// ([`reflection_default_construct`]) is invokable from here.
    #[derive(Debug, Clone, Default)]
    struct ConstructorSummary {
        param_type_names: Vec<String>,
    }

    /// Result of [`reflection_invoke_method0`].
    #[derive(Debug, Clone, Default)]
    struct MethodInvokeResult {
        ok: bool,
        /// Populated when `ok` is `false` (no such zero-arg method, size mismatch, unsupported
        /// return type, or the real call threw -- see `InvokeException`).
        error: String,
        return_type_name: String,
        return_is_void: bool,
        /// Only meaningful when the return type is `Trivial` (see `primitive_kind`'s doc comment on
        /// [`FieldSummary`]).
        primitive_kind: PrimitiveKind,
        /// The return value's raw bytes; empty when `return_is_void` or `!ok`.
        data: Vec<u8>,
    }

    /// Describes a non-trivial [`FieldSummary`]'s container/map/set/optional shape, when it has
    /// one. See [`reflection_field_container`].
    #[derive(Debug, Clone, Default)]
    struct ContainerSummary {
        kind: ContainerKind,
        /// Element type's canonical name (value type, for `Map`); empty when unresolvable.
        element_type_name: String,
        element_primitive_kind: PrimitiveKind,
        /// Only meaningful when `kind == Map`.
        key_type_name: String,
        key_primitive_kind: PrimitiveKind,
    }

    /// One `SFT::Reflection::Attribute` (arbitrary tooling/mod-facing metadata on a type, field,
    /// method, or event). Exactly one of the `*_value` fields is meaningful, selected by `kind`.
    #[derive(Debug, Clone, Default)]
    struct AttributeSummary {
        name: String,
        kind: AttributeKind,
        bool_value: bool,
        int_value: i64,
        float_value: f64,
        string_value: String,
    }

    /// One event (`SFT::Reflection::EventInfo`) a type declares via `SFT_REFLECT_EVENT`.
    /// Descriptor-only -- see the module doc comment for why live subscription was not attempted.
    #[derive(Debug, Clone, Default)]
    struct EventSummary {
        name: String,
        param_type_names: Vec<String>,
    }

    unsafe extern "C++" {
        include!("sturdy_rs/reflection.hpp");

        /// Every currently-registered type's canonical name, in registration order. A linear scan
        /// over the registry (`TypeRegistry::for_each_type`) -- fine for tooling/startup use, not a
        /// hot path.
        fn reflection_type_names() -> Vec<String>;

        /// Looks up a registered type by its canonical name. Returns `false` (and leaves `out`
        /// untouched) when no such type is registered.
        fn reflection_find_type(name: &str, out: &mut TypeSummary) -> bool;
        /// This type's own declared fields (not inherited ones -- matches `TypeInfo::fields`, the
        /// declared-only view; walk `TypeSummary::base_name` yourself to reach ancestors). Empty
        /// (not an error) when `name` is not a registered type.
        fn reflection_type_fields(name: &str) -> Vec<FieldSummary>;

        /// Looks up a registered enum by its canonical name. Returns `false` (and leaves `out`
        /// untouched) when no such enum is registered.
        fn reflection_find_enum(name: &str, out: &mut EnumSummary) -> bool;
        /// This enum's name/value pairs, in declaration order. Empty (not an error) when `name` is
        /// not a registered enum.
        fn reflection_enum_values(name: &str) -> Vec<EnumeratorSummary>;

        /// This type's own declared methods and static methods (`TypeInfo::methods` +
        /// `static_methods`), in declaration order. Empty (not an error) when `name` is not a
        /// registered type.
        fn reflection_type_methods(name: &str) -> Vec<MethodSummary>;
        /// This type's own declared parameterized constructors (`TypeInfo::constructors`). Empty
        /// (not an error) when `name` is not a registered type or declares none.
        fn reflection_type_constructors(name: &str) -> Vec<ConstructorSummary>;

        /// Default-constructs an instance of `name` into `out`, which must be exactly the type's
        /// registered `size` (see [`TypeSummary::size`]/`has_default_constructor`). Returns `false`
        /// when `name` is not a registered type, `out`'s length doesn't match, or the type has no
        /// (nothrow) default constructor.
        ///
        /// The caller owns the resulting bytes and must eventually pass them to
        /// [`reflection_destroy_instance`] if the type owns any non-trivial resources (heap-backed
        /// members, ...) -- this only placement-constructs; it never frees `out` itself (`out` is a
        /// borrowed Rust-owned buffer, not a C++ allocation).
        fn reflection_default_construct(name: &str, out: &mut [u8]) -> bool;
        /// Runs `name`'s destructor in place over `instance`, which must be exactly the type's
        /// registered size and must currently hold a live instance (e.g. produced by
        /// [`reflection_default_construct`]). Returns `false` on a name/size mismatch.
        fn reflection_destroy_instance(name: &str, instance: &mut [u8]) -> bool;

        /// Invokes the zero-argument method (or static method) named `method_name` on `type_name`,
        /// with `object` as the receiver (ignored for a static method, but still validated against
        /// the type's size so a caller can't pass a mismatched buffer).
        ///
        /// Only succeeds when the method truly takes no arguments and its return type is either
        /// `void` or one this binding can size ahead of time (a registered `TypeInfo`, or a
        /// fixed-vocabulary fundamental like `i32`/`f32`/`bool`/...) -- see the module doc comment.
        /// A non-`void`, non-primitive-sized struct return still reports `ok == false` with a
        /// descriptive `error` rather than attempting a partial/unsafe copy.
        fn reflection_invoke_method0(type_name: &str, method_name: &str, object: &mut [u8]) -> MethodInvokeResult;

        /// Classifies `field_name` on `type_name` as a container/map/set/optional and reports its
        /// element (and key, for a map) type name, when it has one of those shapes
        /// (`FieldSummary::is_container`/`is_map`/`is_set`/`is_optional`). Returns `false` (and
        /// leaves `out` untouched) when the type/field doesn't exist or the field has none of those
        /// shapes.
        fn reflection_field_container(type_name: &str, field_name: &str, out: &mut ContainerSummary) -> bool;

        /// Reads a static field's current value (`FieldInfo::static_address`, not any particular
        /// instance's bytes -- see [`FieldSummary::is_static`]). `out` must be exactly the field's
        /// declared size. Returns `false` when the type/field doesn't exist, the field is not
        /// static, or it has no readable (`Trivial` or `copy_get`-backed) representation.
        fn reflection_get_static_field(type_name: &str, field_name: &str, out: &mut [u8]) -> bool;
        /// Writes a static field's value in place. Returns `false` on the same conditions as
        /// [`reflection_get_static_field`], plus when the field is [`FieldSummary::is_read_only`]
        /// or `data`'s length doesn't match the field's declared size.
        fn reflection_set_static_field(type_name: &str, field_name: &str, data: &[u8]) -> bool;

        /// Current element count of `field_name` on `type_name`, read out of `instance`'s bytes at
        /// that field's offset. `0` when the type/field doesn't exist, the field is not a
        /// `Vector`-shaped container (see [`ContainerKind`]), or `instance` is too short to reach
        /// the field.
        fn reflection_container_size(type_name: &str, field_name: &str, instance: &[u8]) -> u64;
        /// Reads the element at `index` (`< reflection_container_size(..)`) into `out`, which must
        /// be exactly the container's element size. Only supported when the element type is
        /// trivially copyable (`ContainerSummary::element_primitive_kind` meaningful, or a
        /// trivially-copyable registered struct) -- see the module doc comment. Returns `false` on
        /// any of those conditions failing, or an out-of-range `index`.
        fn reflection_container_get_element(
            type_name: &str,
            field_name: &str,
            instance: &[u8],
            index: u64,
            out: &mut [u8],
        ) -> bool;
        /// Writes `data` into the element at `index`. Same size/triviality/range requirements as
        /// [`reflection_container_get_element`].
        fn reflection_container_set_element(
            type_name: &str,
            field_name: &str,
            instance: &mut [u8],
            index: u64,
            data: &[u8],
        ) -> bool;
        /// Resizes the container to exactly `new_size` elements (default-constructing new ones,
        /// destroying truncated ones). Returns `false` when the type/field doesn't exist, the field
        /// is not a `Vector`-shaped container, the element type has no default constructor, or
        /// `instance` is too short to reach the field.
        fn reflection_container_resize(type_name: &str, field_name: &str, instance: &mut [u8], new_size: u64) -> bool;

        /// `name`'s own attributes (`TypeInfo::attributes`). Empty (not an error) when `name` is not
        /// a registered type or declares none.
        fn reflection_type_attributes(name: &str) -> Vec<AttributeSummary>;
        /// `field_name`'s own attributes (`FieldInfo::attributes`) on `type_name`. Empty (not an
        /// error) when the type/field doesn't exist or declares none.
        fn reflection_field_attributes(type_name: &str, field_name: &str) -> Vec<AttributeSummary>;

        /// `name`'s own declared events (`TypeInfo::events`), in declaration order. Empty (not an
        /// error) when `name` is not a registered type or declares none. Descriptor-only -- see the
        /// module doc comment for why subscription is out of scope.
        fn reflection_type_events(name: &str) -> Vec<EventSummary>;
        /// Looks up one event by name (rather than enumerating all of them). Returns `false` (and
        /// leaves `out` untouched) when the type/event doesn't exist.
        fn reflection_find_event(type_name: &str, event_name: &str, out: &mut EventSummary) -> bool;
        /// Fires `event_name` on `object` (an instance of `type_name`; `object`'s bytes are the
        /// receiver passed to every listener, unused but still size-validated for a static-only
        /// event) with the given arguments, calling every currently-subscribed listener in
        /// registration order -- a safe no-op when nobody has subscribed (subscribing itself is out
        /// of scope; see the module doc comment).
        ///
        /// `arg_sizes`/`args_data` mirror `reflection_default_construct`'s convention elsewhere in
        /// this bridge: `args_data` is every argument's bytes concatenated in
        /// [`EventSummary::param_type_names`] order, `arg_sizes` the parallel per-argument byte
        /// count. Returns `false` (firing nothing) unless `arg_sizes.len()` matches the event's
        /// declared arity *and* every size matches that parameter's own registered/fundamental
        /// size exactly -- this is the only thing standing between a caller-supplied byte layout
        /// and an out-of-bounds read on the C++ side, so it is enforced strictly rather than
        /// best-effort.
        fn reflection_fire_event(
            type_name: &str,
            event_name: &str,
            object: &mut [u8],
            arg_sizes: &[u32],
            args_data: &[u8],
        ) -> bool;

        /// Registers this crate's own tiny demo type (`sturdy_rs::reflection::demo::DemoActor`,
        /// canonical name `"sturdy_rs.demo.actor"`) with the process-wide registry, if it isn't
        /// already. Exists because the vendored engine ships no small, stable, gameplay-shaped
        /// reflected type of its own to demonstrate this module's live-instance/method/container
        /// surface against -- see `sturdy_rs/reflection.cpp` for the type's actual field/method/
        /// event declarations. Idempotent and cheap to call more than once
        /// (`TypeRegistry::type<T>()`'s own lazy-registration is idempotent).
        fn reflection_register_demo_type();

        /// Byte layout of a method's signature, so a Rust hook/override can slice the raw `args`/
        /// return pointers it's handed. `ok == false` if the type/method doesn't exist or any
        /// parameter/return type's size can't be resolved (registered type or fundamental).
        fn reflection_method_layout(type_name: &str, method_name: &str) -> CallableLayout;
        /// Same as [`reflection_method_layout`], for an event's parameters (`return_size` is 0).
        fn reflection_event_layout(type_name: &str, event_name: &str) -> CallableLayout;

        /// Replaces `method_name`'s body with `callback` (`TypeRegistry::set_method_override`)
        /// for every subsequent reflected invocation. At most one override per method; a new one
        /// replaces the old. Returns `false` for an unknown type/method.
        ///
        /// # Safety
        /// `callback` must stay alive until the override is cleared *and* no invocation can
        /// still be in flight (the binding leaks it for exactly that reason).
        // clippy's `missing_safety_doc` can't see doc comments on an `unsafe fn` declared inside a
        // `#[cxx::bridge]` block (confirmed empirically: it still fires with the `# Safety` section
        // right above, on every `unsafe fn` in this file and in `schedule.rs`'s bridge) -- silenced
        // per-fn rather than module-wide so a real future doc gap on a *safe* fn still gets caught.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn reflection_set_method_override(
            type_name: &str,
            method_name: &str,
            callback: *mut RustReflectionCallback,
        ) -> bool;
        fn reflection_clear_method_override(type_name: &str, method_name: &str) -> bool;
        /// Adds a before (`after == false`) or after hook; any number may coexist. Returns the
        /// subscription id, or 0 for an unknown type/method.
        ///
        /// # Safety
        /// Same lifetime contract as [`reflection_set_method_override`].
        // clippy false positive, see `reflection_set_method_override` above.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn reflection_add_method_hook(
            type_name: &str,
            method_name: &str,
            after: bool,
            callback: *mut RustReflectionCallback,
        ) -> u64;
        fn reflection_remove_method_hook(type_name: &str, method_name: &str, after: bool, subscription: u64) -> bool;
        /// Subscribes `callback` to an event; fired by `reflection_fire_event` and by any C++
        /// `SFT_REFLECT_FIRE_EVENT`. Returns the subscription id, or 0 for an unknown type/event.
        ///
        /// # Safety
        /// Same lifetime contract as [`reflection_set_method_override`].
        // clippy false positive, see `reflection_set_method_override` above.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn reflection_subscribe_event(
            type_name: &str,
            event_name: &str,
            callback: *mut RustReflectionCallback,
        ) -> u64;
        fn reflection_unsubscribe_event(type_name: &str, event_name: &str, subscription: u64) -> bool;

        /// Byte size of `type_name` when it's a registered type or a fundamental (`"i32"`,
        /// `"f32"`, ...); 0 when unknown (or `"void"`).
        fn reflection_type_size(type_name: &str) -> u64;

        /// An in-progress runtime type (`TypeInfoBuilder`), registered by
        /// [`reflection_type_builder_finish`] and discarded (freeing any pending method callbacks)
        /// when dropped unfinished.
        ///
        /// Runtime types are plain bytes: move/copy are `memcpy`, destroy is a no-op, and default
        /// construction copies `default_bytes`. Field types are restricted to fundamentals and
        /// other runtime-registered types, so that stays sound.
        type DynamicTypeBuilder;

        /// Starts a runtime type. `default_bytes` must be empty (zeroed default) or exactly
        /// `size` long; `align` must be a nonzero power of two.
        fn reflection_type_builder_new(
            name: &str,
            size: u64,
            align: u64,
            default_bytes: &[u8],
        ) -> UniquePtr<DynamicTypeBuilder>;
        /// Declares a trivial field. Returns an error message, or empty on success.
        fn reflection_type_builder_add_field(
            builder: Pin<&mut DynamicTypeBuilder>,
            name: &str,
            offset: u64,
            type_name: &str,
            attributes: &Vec<AttributeSummary>,
        ) -> String;
        /// Declares a method whose body is `callback`. `return_type_name` is `"void"` (or empty)
        /// for none. Takes ownership of `callback` even on failure. Returns an error message, or
        /// empty on success.
        ///
        /// # Safety
        /// `callback` must be a uniquely owned `Box<RustReflectionCallback>` pointer.
        // clippy false positive, see `reflection_set_method_override` above.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn reflection_type_builder_add_method(
            builder: Pin<&mut DynamicTypeBuilder>,
            name: &str,
            return_type_name: &str,
            param_type_names: &Vec<String>,
            callback: *mut RustReflectionCallback,
        ) -> String;
        /// Declares an event. Returns an error message, or empty on success.
        fn reflection_type_builder_add_event(
            builder: Pin<&mut DynamicTypeBuilder>,
            name: &str,
            param_type_names: &Vec<String>,
        ) -> String;
        fn reflection_type_builder_add_type_attribute(builder: Pin<&mut DynamicTypeBuilder>, attribute: &AttributeSummary);
        /// Registers the type (`TypeRegistry::register_type`). Returns an error message, or empty
        /// on success. Fails if the name is already registered.
        fn reflection_type_builder_finish(builder: UniquePtr<DynamicTypeBuilder>) -> String;
        /// `TypeRegistry::unregister_type` by name. `false` if nothing was registered under it.
        /// Pointers other code already resolved stay valid (the registry never frees
        /// descriptors), but name lookups stop finding it.
        fn reflection_unregister_type(type_name: &str) -> bool;

        /// Invokes a method with byte-marshalled arguments (`TypeRegistry::invoke_method`, so
        /// overrides and hooks apply). Every parameter and the return type must be a
        /// fundamental or a runtime-registered type (plain bytes); each `args` chunk must match
        /// its parameter's size exactly. `out_return` must be exactly the return size (empty for
        /// `void`). Returns an error message, or empty on success.
        fn reflection_invoke_method(
            type_name: &str,
            method_name: &str,
            object: &mut [u8],
            arg_sizes: &[u32],
            args_data: &[u8],
            out_return: &mut [u8],
        ) -> String;

        /// Sets the runtime display-name override (`TypeRegistry::set_{type,field,method}_name_override`)
        /// of a type (`member` ignored), or of one of its declared fields/methods. Descriptors
        /// themselves are untouched; only the `effective_*` views change. `false` if not found.
        fn reflection_overlay_set_name(target: OverlayTarget, type_name: &str, member: &str, name: &str) -> bool;
        /// Clears a name override; `false` if there was none (or not found).
        fn reflection_overlay_clear_name(target: OverlayTarget, type_name: &str, member: &str) -> bool;
        /// Attaches an extra attribute, additive to the static ones. `false` if not found.
        fn reflection_overlay_add_attribute(
            target: OverlayTarget,
            type_name: &str,
            member: &str,
            attribute: &AttributeSummary,
        ) -> bool;
        /// Removes every overlay (name and attributes); `false` if there was none (or not found).
        fn reflection_overlay_clear(target: OverlayTarget, type_name: &str, member: &str) -> bool;
        /// Effective name (override, else the static name). `false` if not found.
        fn reflection_overlay_effective_name(target: OverlayTarget, type_name: &str, member: &str, out: &mut String) -> bool;
        /// Effective attributes (static, then overlay). Empty if not found.
        fn reflection_overlay_effective_attributes(
            target: OverlayTarget,
            type_name: &str,
            member: &str,
        ) -> Vec<AttributeSummary>;
    }

    /// What a `reflection_overlay_*` call addresses.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    enum OverlayTarget {
        Type,
        Field,
        Method,
    }

    /// See [`reflection_method_layout`].
    #[derive(Debug, Clone, Default)]
    struct CallableLayout {
        ok: bool,
        /// Byte size of the owning type (the `object` a hook sees); 0 for an unknown type.
        object_size: u64,
        param_sizes: Vec<u64>,
        /// 0 for `void`.
        return_size: u64,
        is_static: bool,
    }

    extern "Rust" {
        type RustReflectionCallback;

        /// Called by the C++ trampolines behind overrides, hooks, and event subscriptions.
        /// `out_return` is null for hooks/events and for `void` overrides.
        ///
        /// # Safety
        /// `callback` must be a live `RustReflectionCallback`; `object`/`args`/`out_return` are
        /// exactly what the engine passed (sized per [`reflection_method_layout`]).
        unsafe fn rust_reflection_callback_run(
            callback: *mut RustReflectionCallback,
            object: *mut u8,
            args: *const *const u8,
            out_return: *mut u8,
        );

        /// Frees a callback that never got installed (a discarded [`DynamicTypeBuilder`]).
        ///
        /// # Safety
        /// `callback` must be a uniquely owned `Box<RustReflectionCallback>` pointer.
        unsafe fn rust_reflection_callback_drop(callback: *mut RustReflectionCallback);
    }
}

/// A boxed Rust closure behind a reflection override/hook/event subscription. The engine may
/// invoke it from any thread, possibly concurrently, hence `Fn + Send + Sync`.
pub struct RustReflectionCallback {
    pub body: Box<dyn Fn(*mut u8, *const *const u8, *mut u8) + Send + Sync>,
}

/// # Safety
/// See the bridge declaration.
unsafe fn rust_reflection_callback_run(
    callback: *mut RustReflectionCallback,
    object: *mut u8,
    args: *const *const u8,
    out_return: *mut u8,
) {
    crate::diagnostics::guard("reflection callback", || unsafe { ((*callback).body)(object, args, out_return) });
}

/// # Safety
/// See the bridge declaration.
unsafe fn rust_reflection_callback_drop(callback: *mut RustReflectionCallback) {
    drop(unsafe { Box::from_raw(callback) });
}
