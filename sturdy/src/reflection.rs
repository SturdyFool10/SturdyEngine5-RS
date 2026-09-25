//! Type introspection *and* limited live-instance access over the engine's process-wide reflection
//! registry.
//!
//! Unlike the rest of this crate, nothing here takes an [`Engine`](crate::Engine): the registry is
//! a singleton, populated by whatever engine/game code registered reflected types before this is
//! called (typically during startup) and queryable from any thread.
//!
//! ```no_run
//! use sturdy::reflection;
//!
//! for name in reflection::type_names() {
//!     if let Some(info) = reflection::find_type(&name) {
//!         println!("{name}: {} bytes, {} fields", info.size, info.field_count);
//!     }
//! }
//! ```
//!
//! Beyond the original type/field/enum descriptors, this module also covers (see
//! `sturdy-sys/src/reflection.rs`'s module doc comment for the full rationale of each):
//!  * Live field read/write against a byte view of an actual instance ([`read_field`]/
//!    [`write_field`]/[`read_field_as`]/[`write_field_value`]) -- pure offset arithmetic against
//!    [`Field::offset`]/[`Field::size`], no C++ call involved.
//!  * Default-constructing (and destroying) an instance of a registered type ([`construct_default`],
//!    [`Instance`]).
//!  * Method descriptors ([`methods`]) and invoking a zero-argument method ([`invoke_method0`]).
//!  * Constructor descriptors ([`constructors`]) -- enumeration only; see [`invoke_method0`]'s
//!    sibling doc comment on why arbitrary-argument invocation isn't attempted.
//!  * Container/map/set/optional element-shape introspection for a non-trivial field
//!    ([`field_container`]), plus element read/write/resize for a `Vector`-shaped one
//!    ([`container_len`]/[`container_get_element_as`]/[`container_set_element_value`]/
//!    [`container_resize`]) -- unlike field read/write, this *does* make an engine call (element
//!    access needs real `ContainerInfo` function pointers, not just offset math).
//!  * Attribute enumeration on a type or field ([`type_attributes`]/[`field_attributes`]).
//!  * Event descriptors ([`events`]/[`find_event`]), firing one ([`fire_event`]), and subscribing
//!    a Rust closure ([`subscribe_event`]).
//!  * Method overrides ([`override_method`]) and before/after hooks ([`add_method_hook`]) -- Rust
//!    closures that run on every reflected invocation, from Rust or C++, until their handle drops.
//!  * Static field read/write ([`get_static_field`]/[`set_static_field`]/
//!    [`get_static_field_as`]/[`set_static_field_value`]) -- unlike instance fields, this needs a
//!    real engine call too (a static field's storage has no per-instance byte view to slice into).

use sturdy_sys::reflection::ffi;

/// Which scalar shape a [`Field`]'s (or [`Enumerator`]'s underlying type's) bytes represent, when
/// [`Field::is_trivial`] is `true`. Mirrors `SFT::Reflection::PrimitiveKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveKind {
    /// Not a recognized scalar primitive (a struct, container, map, ...).
    None,
    Bool,
    SignedInt,
    UnsignedInt,
    Float,
}

impl From<ffi::PrimitiveKind> for PrimitiveKind {
    fn from(value: ffi::PrimitiveKind) -> Self {
        match value {
            ffi::PrimitiveKind::Bool => Self::Bool,
            ffi::PrimitiveKind::SignedInt => Self::SignedInt,
            ffi::PrimitiveKind::UnsignedInt => Self::UnsignedInt,
            ffi::PrimitiveKind::Float => Self::Float,
            _ => Self::None,
        }
    }
}

/// A registered type's descriptor (`SFT::Reflection::TypeInfo`, minus methods/events/constructors
/// /attributes -- see the module doc comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub size: usize,
    pub align: usize,
    /// Number of instance fields declared directly on this type (not inherited).
    pub field_count: usize,
    /// Number of instance methods declared directly on this type; provided as a count only --
    /// enumerating methods is out of scope for this pass.
    pub method_count: usize,
    /// The primary reflected base's name, if any. Walk this yourself (`find_type` again) to reach
    /// further ancestors.
    pub base_name: Option<String>,
    /// Number of additional (non-primary) reflected bases (`TypeInfo::secondary_bases`).
    pub secondary_base_count: usize,
    /// Whether this type can be default-constructed through [`construct_default`].
    pub has_default_constructor: bool,
}

impl From<ffi::TypeSummary> for TypeInfo {
    fn from(value: ffi::TypeSummary) -> Self {
        Self {
            name: value.name,
            size: value.size as usize,
            align: value.align as usize,
            field_count: value.field_count as usize,
            method_count: value.method_count as usize,
            base_name: value.has_base.then_some(value.base_name),
            secondary_base_count: value.secondary_base_count as usize,
            has_default_constructor: value.has_default_constructor,
        }
    }
}

/// One field of a [`TypeInfo`] (`SFT::Reflection::FieldInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// The field's declared type's canonical name, when that type is itself registered (a nested
    /// reflected struct/enum usually is; many raw primitives are not).
    pub field_type_name: Option<String>,
    /// Byte offset within the owning type.
    pub offset: usize,
    pub size: usize,
    pub align: usize,
    pub is_static: bool,
    pub is_read_only: bool,
    /// Trivially copyable -- safe to `memcpy`; see [`Field::primitive_kind`] for its scalar shape.
    pub is_trivial: bool,
    /// `std::vector<T>`-shaped.
    pub is_container: bool,
    /// `std::unordered_map<K, V>`-shaped.
    pub is_map: bool,
    /// `std::set<T>`/`std::unordered_set<T>`-shaped.
    pub is_set: bool,
    /// `std::optional<T>`/`std::unique_ptr<T>`/`std::shared_ptr<T>`-shaped.
    pub is_optional: bool,
    pub primitive_kind: PrimitiveKind,
}

impl From<ffi::FieldSummary> for Field {
    fn from(value: ffi::FieldSummary) -> Self {
        Self {
            name: value.name,
            field_type_name: (!value.field_type_name.is_empty()).then_some(value.field_type_name),
            offset: value.offset as usize,
            size: value.size as usize,
            align: value.align as usize,
            is_static: value.is_static,
            is_read_only: value.is_read_only,
            is_trivial: value.is_trivial,
            is_container: value.is_container,
            is_map: value.is_map,
            is_set: value.is_set,
            is_optional: value.is_optional,
            primitive_kind: value.primitive_kind.into(),
        }
    }
}

/// A registered enum's descriptor (`SFT::Reflection::EnumInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumType {
    pub name: String,
    pub size: usize,
    pub align: usize,
    /// The underlying integer type's canonical name, when that type is itself registered (usually
    /// not, for a plain `i32`/`u8`/...).
    pub underlying_type_name: Option<String>,
    pub enumerator_count: usize,
}

impl From<ffi::EnumSummary> for EnumType {
    fn from(value: ffi::EnumSummary) -> Self {
        Self {
            name: value.name,
            size: value.size as usize,
            align: value.align as usize,
            underlying_type_name: (!value.underlying_type_name.is_empty()).then_some(value.underlying_type_name),
            enumerator_count: value.enumerator_count as usize,
        }
    }
}

/// One name/value pair of an [`EnumType`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enumerator {
    pub name: String,
    pub value: i64,
}

impl From<ffi::EnumeratorSummary> for Enumerator {
    fn from(value: ffi::EnumeratorSummary) -> Self {
        Self { name: value.name, value: value.value }
    }
}

/// Every currently-registered type's canonical name, in registration order. A linear scan over the
/// registry -- fine for tooling/startup use, not a hot path.
pub fn type_names() -> Vec<String> {
    ffi::reflection_type_names()
}

/// Looks up a registered type by its canonical name.
pub fn find_type(name: &str) -> Option<TypeInfo> {
    let mut out = ffi::TypeSummary::default();
    ffi::reflection_find_type(name, &mut out).then(|| out.into())
}

/// This type's own declared fields (not inherited ones -- walk [`TypeInfo::base_name`] yourself to
/// reach ancestors). Empty (not an error) when `name` is not a registered type.
pub fn fields(name: &str) -> Vec<Field> {
    ffi::reflection_type_fields(name).into_iter().map(Field::from).collect()
}

/// Looks up a registered enum by its canonical name.
pub fn find_enum(name: &str) -> Option<EnumType> {
    let mut out = ffi::EnumSummary::default();
    ffi::reflection_find_enum(name, &mut out).then(|| out.into())
}

/// This enum's name/value pairs, in declaration order. Empty (not an error) when `name` is not a
/// registered enum.
pub fn enum_values(name: &str) -> Vec<Enumerator> {
    ffi::reflection_enum_values(name).into_iter().map(Enumerator::from).collect()
}

/// Registers this crate's own tiny demo reflected type (canonical name `"sturdy_rs.demo.actor"`),
/// if it isn't already registered. See `sturdy_sys::reflection::ffi::reflection_register_demo_type`
/// for why it exists: the vendored engine ships no small, stable, gameplay-shaped reflected type of
/// its own to exercise the live-instance/method/container surface below against.
pub fn register_demo_type() {
    ffi::reflection_register_demo_type();
}

// -------------------------------------------------------------------------------------------
// Live instance field read/write
// -------------------------------------------------------------------------------------------

/// Why a [`read_field`]/[`write_field`] call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldAccessError {
    /// No field with that name among the fields passed in.
    NotFound,
    /// The field is not `Trivial` (see [`Field::is_trivial`]) -- its bytes cannot be read/written
    /// generically; a struct-shaped field needs its own recursive [`fields`] walk instead.
    NotTrivial,
    /// [`write_field`] only: the field is [`Field::is_read_only`].
    ReadOnly,
    /// [`write_field`] only: the supplied value's length didn't match [`Field::size`].
    SizeMismatch { expected: usize, actual: usize },
    /// The instance byte slice is too short to contain this field at its declared offset --
    /// almost always means the wrong `fields` list (from a different type) was passed in.
    InstanceTooSmall { field_end: usize, instance_len: usize },
    /// [`get_static_field`]/[`set_static_field`] only: the named field exists but is not
    /// [`Field::is_static`].
    NotStatic,
    /// [`get_static_field`]/[`set_static_field`] only: the field was static and otherwise
    /// locally valid (right name, right size, writable), but the engine still refused the
    /// read/write -- means it has no readable/writable representation (not `Trivial`, no
    /// `copy_get`/`copy_set`).
    Rejected,
}

impl core::fmt::Display for FieldAccessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FieldAccessError::NotFound => write!(f, "no such field"),
            FieldAccessError::NotTrivial => write!(f, "field is not trivially copyable"),
            FieldAccessError::ReadOnly => write!(f, "field is read-only"),
            FieldAccessError::SizeMismatch { expected, actual } => {
                write!(f, "value size {actual} does not match the field's size {expected}")
            }
            FieldAccessError::InstanceTooSmall { field_end, instance_len } => write!(
                f,
                "instance buffer is {instance_len} bytes, but the field ends at byte {field_end}"
            ),
            FieldAccessError::NotStatic => write!(f, "field is not static"),
            FieldAccessError::Rejected => {
                write!(f, "the engine refused this read/write (no trivial/copy_get/copy_set representation)")
            }
        }
    }
}

impl std::error::Error for FieldAccessError {}

/// Reads a trivial field's raw bytes out of `instance`'s byte view, given `type_name`'s own field
/// list (from [`fields`]) and the field's name.
///
/// `instance` is whatever byte view the caller already legitimately has -- an ECS component's bytes
/// via [`World::read_component`](crate::World::read_component), an [`Instance`]'s bytes, or
/// anything else the caller can vouch is really a `type_name`-shaped instance starting at
/// `instance[0]`. This function performs no engine call: it is pure offset+bounds-checked slicing
/// against [`Field::offset`]/[`Field::size`].
pub fn read_field<'a>(fields: &[Field], field_name: &str, instance: &'a [u8]) -> Result<&'a [u8], FieldAccessError> {
    let field = fields.iter().find(|f| f.name == field_name).ok_or(FieldAccessError::NotFound)?;
    if !field.is_trivial {
        return Err(FieldAccessError::NotTrivial);
    }
    let end = field.offset + field.size;
    if end > instance.len() {
        return Err(FieldAccessError::InstanceTooSmall { field_end: end, instance_len: instance.len() });
    }
    Ok(&instance[field.offset..end])
}

/// Writes `value`'s bytes into a trivial field in place, given `type_name`'s own field list (from
/// [`fields`]) and the field's name. See [`read_field`] for what `instance` needs to be.
pub fn write_field(fields: &[Field], field_name: &str, instance: &mut [u8], value: &[u8]) -> Result<(), FieldAccessError> {
    let field = fields.iter().find(|f| f.name == field_name).ok_or(FieldAccessError::NotFound)?;
    if !field.is_trivial {
        return Err(FieldAccessError::NotTrivial);
    }
    if field.is_read_only {
        return Err(FieldAccessError::ReadOnly);
    }
    if value.len() != field.size {
        return Err(FieldAccessError::SizeMismatch { expected: field.size, actual: value.len() });
    }
    let end = field.offset + field.size;
    if end > instance.len() {
        return Err(FieldAccessError::InstanceTooSmall { field_end: end, instance_len: instance.len() });
    }
    instance[field.offset..end].copy_from_slice(value);
    Ok(())
}

/// Typed convenience over [`read_field`]: reads the field's bytes and decodes them as `T` via
/// `bytemuck`. Fails with [`FieldAccessError::SizeMismatch`] if `T`'s size doesn't match the
/// field's declared size (e.g. the caller guessed the wrong Rust type for a C++ `int` field).
pub fn read_field_as<T: bytemuck::Pod>(fields: &[Field], field_name: &str, instance: &[u8]) -> Result<T, FieldAccessError> {
    let bytes = read_field(fields, field_name, instance)?;
    if bytes.len() != core::mem::size_of::<T>() {
        return Err(FieldAccessError::SizeMismatch { expected: bytes.len(), actual: core::mem::size_of::<T>() });
    }
    Ok(bytemuck::pod_read_unaligned(bytes))
}

/// Typed convenience over [`write_field`]: encodes `value` via `bytemuck` and writes its bytes.
pub fn write_field_value<T: bytemuck::Pod>(
    fields: &[Field],
    field_name: &str,
    instance: &mut [u8],
    value: T,
) -> Result<(), FieldAccessError> {
    write_field(fields, field_name, instance, bytemuck::bytes_of(&value))
}

// -------------------------------------------------------------------------------------------
// Static field read/write
// -------------------------------------------------------------------------------------------

/// Reads a static field's current value (`type_name`'s own true storage, not any particular
/// instance's bytes -- unlike [`read_field`], this makes a real engine call, since a static
/// field's address lives in the C++ process with no Rust-visible byte view to slice into).
pub fn get_static_field(fields: &[Field], type_name: &str, field_name: &str) -> Result<Vec<u8>, FieldAccessError> {
    let field = fields.iter().find(|f| f.name == field_name).ok_or(FieldAccessError::NotFound)?;
    if !field.is_static {
        return Err(FieldAccessError::NotStatic);
    }
    let mut out = vec![0u8; field.size];
    if ffi::reflection_get_static_field(type_name, field_name, &mut out) {
        Ok(out)
    } else {
        Err(FieldAccessError::Rejected)
    }
}

/// Writes a static field's value in place. See [`get_static_field`] for why this makes a real
/// engine call rather than slicing into a byte view.
pub fn set_static_field(
    fields: &[Field],
    type_name: &str,
    field_name: &str,
    value: &[u8],
) -> Result<(), FieldAccessError> {
    let field = fields.iter().find(|f| f.name == field_name).ok_or(FieldAccessError::NotFound)?;
    if !field.is_static {
        return Err(FieldAccessError::NotStatic);
    }
    if field.is_read_only {
        return Err(FieldAccessError::ReadOnly);
    }
    if value.len() != field.size {
        return Err(FieldAccessError::SizeMismatch { expected: field.size, actual: value.len() });
    }
    if ffi::reflection_set_static_field(type_name, field_name, value) {
        Ok(())
    } else {
        Err(FieldAccessError::Rejected)
    }
}

/// Typed convenience over [`get_static_field`], decoding via `bytemuck`.
pub fn get_static_field_as<T: bytemuck::Pod>(
    fields: &[Field],
    type_name: &str,
    field_name: &str,
) -> Result<T, FieldAccessError> {
    let bytes = get_static_field(fields, type_name, field_name)?;
    if bytes.len() != core::mem::size_of::<T>() {
        return Err(FieldAccessError::SizeMismatch { expected: bytes.len(), actual: core::mem::size_of::<T>() });
    }
    Ok(bytemuck::pod_read_unaligned(&bytes))
}

/// Typed convenience over [`set_static_field`], encoding via `bytemuck`.
pub fn set_static_field_value<T: bytemuck::Pod>(
    fields: &[Field],
    type_name: &str,
    field_name: &str,
    value: T,
) -> Result<(), FieldAccessError> {
    set_static_field(fields, type_name, field_name, bytemuck::bytes_of(&value))
}

// -------------------------------------------------------------------------------------------
// Constructors
// -------------------------------------------------------------------------------------------

/// A registered type's parameterized constructor descriptor (`SFT::Reflection::ConstructorInfo`).
/// Enumeration only -- see the crate's `sturdy-sys` reflection module doc comment for why
/// arbitrary-argument construction isn't invokable from here; use [`construct_default`] for the
/// zero-argument case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorDescriptor {
    pub param_type_names: Vec<String>,
}

impl From<ffi::ConstructorSummary> for ConstructorDescriptor {
    fn from(value: ffi::ConstructorSummary) -> Self {
        Self { param_type_names: value.param_type_names }
    }
}

/// `name`'s own declared parameterized constructors. Empty (not an error) when `name` is not a
/// registered type or declares none.
pub fn constructors(name: &str) -> Vec<ConstructorDescriptor> {
    ffi::reflection_type_constructors(name).into_iter().map(ConstructorDescriptor::from).collect()
}

/// An owned, live instance of a registered type's bytes, produced by [`construct_default`]. Runs
/// the type's real C++ destructor in place when dropped, so a type owning non-trivial resources
/// (a `std::vector`-backed field, ...) doesn't leak.
pub struct Instance {
    type_name: String,
    bytes: Vec<u8>,
}

impl Instance {
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Best-effort: the only way this can fail is a name/size mismatch, which cannot happen
        // for bytes this type itself constructed.
        let _ = ffi::reflection_destroy_instance(&self.type_name, &mut self.bytes);
    }
}

/// Default-constructs a new instance of `type_name`, if it is a registered type with a (nothrow)
/// default constructor (see [`TypeInfo::has_default_constructor`]). `None` otherwise.
pub fn construct_default(type_name: &str) -> Option<Instance> {
    let info = find_type(type_name)?;
    let mut bytes = vec![0u8; info.size];
    ffi::reflection_default_construct(type_name, &mut bytes).then(|| Instance { type_name: type_name.to_owned(), bytes })
}

// -------------------------------------------------------------------------------------------
// Methods
// -------------------------------------------------------------------------------------------

/// One method or static method (`SFT::Reflection::MethodInfo`). Enumeration only, except for the
/// zero-argument case -- see [`invoke_method0`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDescriptor {
    pub name: String,
    pub is_static: bool,
    /// Each parameter's canonical type name, in order; empty entries mean that parameter's type
    /// has no name this binding can resolve.
    pub param_type_names: Vec<String>,
    pub return_type_name: String,
    pub return_is_void: bool,
}

impl From<ffi::MethodSummary> for MethodDescriptor {
    fn from(value: ffi::MethodSummary) -> Self {
        Self {
            name: value.name,
            is_static: value.is_static,
            param_type_names: value.param_type_names,
            return_type_name: value.return_type_name,
            return_is_void: value.return_is_void,
        }
    }
}

/// `name`'s own declared methods and static methods, in declaration order. Empty (not an error)
/// when `name` is not a registered type.
pub fn methods(name: &str) -> Vec<MethodDescriptor> {
    ffi::reflection_type_methods(name).into_iter().map(MethodDescriptor::from).collect()
}

/// A zero-argument method's return value, decoded by [`invoke_method0`].
#[derive(Debug, Clone, PartialEq)]
pub enum MethodValue {
    Void,
    Bool(bool),
    SignedInt(i64),
    UnsignedInt(u64),
    Float(f64),
    /// A non-primitive (but sized -- a registered `TypeInfo`) return type's raw bytes, plus its
    /// canonical type name. The caller must already know how to interpret these bytes.
    Bytes { type_name: String, data: Vec<u8> },
}

/// Why [`invoke_method0`] failed: the underlying C++ error message (no such zero-arg method, a
/// buffer size mismatch, an unsupported return type, or the real call threw).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodInvokeError(pub String);

impl core::fmt::Display for MethodInvokeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MethodInvokeError {}

fn decode_signed(bytes: &[u8]) -> i64 {
    match bytes.len() {
        1 => bytes[0] as i8 as i64,
        2 => i16::from_ne_bytes(bytes.try_into().unwrap()) as i64,
        4 => i32::from_ne_bytes(bytes.try_into().unwrap()) as i64,
        8 => i64::from_ne_bytes(bytes.try_into().unwrap()),
        _ => 0,
    }
}

fn decode_unsigned(bytes: &[u8]) -> u64 {
    match bytes.len() {
        1 => bytes[0] as u64,
        2 => u16::from_ne_bytes(bytes.try_into().unwrap()) as u64,
        4 => u32::from_ne_bytes(bytes.try_into().unwrap()) as u64,
        8 => u64::from_ne_bytes(bytes.try_into().unwrap()),
        _ => 0,
    }
}

fn decode_float(bytes: &[u8]) -> f64 {
    match bytes.len() {
        4 => f32::from_ne_bytes(bytes.try_into().unwrap()) as f64,
        8 => f64::from_ne_bytes(bytes.try_into().unwrap()),
        _ => 0.0,
    }
}

/// Invokes the zero-argument method (or static method) named `method_name` on `type_name`, with
/// `object` as the receiver (ignored, but still size-checked, for a static method).
///
/// Only succeeds when the method truly takes no arguments and its return type is either `void` or
/// one this binding can size ahead of time -- see the `sturdy-sys` reflection module doc comment
/// for the full explanation of why arbitrary-signature invocation from a dynamic Rust call site is
/// out of scope in general, and why the zero-argument/primitive-or-known-struct-return case is the
/// one that's solid here.
pub fn invoke_method0(type_name: &str, method_name: &str, object: &mut [u8]) -> Result<MethodValue, MethodInvokeError> {
    let result = ffi::reflection_invoke_method0(type_name, method_name, object);
    if !result.ok {
        return Err(MethodInvokeError(result.error));
    }
    if result.return_is_void {
        return Ok(MethodValue::Void);
    }
    Ok(match PrimitiveKind::from(result.primitive_kind) {
        PrimitiveKind::Bool => MethodValue::Bool(result.data.first().copied().unwrap_or(0) != 0),
        PrimitiveKind::SignedInt => MethodValue::SignedInt(decode_signed(&result.data)),
        PrimitiveKind::UnsignedInt => MethodValue::UnsignedInt(decode_unsigned(&result.data)),
        PrimitiveKind::Float => MethodValue::Float(decode_float(&result.data)),
        PrimitiveKind::None => MethodValue::Bytes { type_name: result.return_type_name, data: result.data },
    })
}

// -------------------------------------------------------------------------------------------
// Container/map/set/optional element introspection
// -------------------------------------------------------------------------------------------

/// Which container-ish shape a non-trivial [`Field`] has, if any. Mirrors
/// `SFT::Reflection::FieldInfo`'s mutually-exclusive `container`/`map`/`set`/`optional` pointers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    Vector,
    Map,
    Set,
    Optional,
}

/// A non-trivial field's element shape, from [`field_container`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerShape {
    pub kind: ContainerKind,
    /// The element type's canonical name (the *value* type, for a [`ContainerKind::Map`]), when
    /// resolvable.
    pub element_type_name: Option<String>,
    pub element_primitive_kind: PrimitiveKind,
    /// Only meaningful for [`ContainerKind::Map`].
    pub key_type_name: Option<String>,
    /// Only meaningful for [`ContainerKind::Map`].
    pub key_primitive_kind: PrimitiveKind,
}

/// Classifies `field_name` on `type_name` as a container/map/set/optional and reports its element
/// (and key, for a map) type, when [`Field::is_container`]/[`Field::is_map`]/[`Field::is_set`]/
/// [`Field::is_optional`] says it has one of those shapes. `None` when the type/field doesn't
/// exist, or the field has none of those shapes (e.g. it's a plain trivial or struct field).
pub fn field_container(type_name: &str, field_name: &str) -> Option<ContainerShape> {
    let mut out = ffi::ContainerSummary::default();
    if !ffi::reflection_field_container(type_name, field_name, &mut out) {
        return None;
    }
    let kind = match out.kind {
        ffi::ContainerKind::Vector => ContainerKind::Vector,
        ffi::ContainerKind::Map => ContainerKind::Map,
        ffi::ContainerKind::Set => ContainerKind::Set,
        ffi::ContainerKind::Optional => ContainerKind::Optional,
        _ => return None,
    };
    Some(ContainerShape {
        kind,
        element_type_name: (!out.element_type_name.is_empty()).then_some(out.element_type_name),
        element_primitive_kind: out.element_primitive_kind.into(),
        key_type_name: (!out.key_type_name.is_empty()).then_some(out.key_type_name),
        key_primitive_kind: out.key_primitive_kind.into(),
    })
}

/// Current element count of a [`ContainerKind::Vector`]-shaped field, read out of `instance`'s
/// bytes at that field's offset. `0` when the type/field doesn't exist, the field isn't
/// `Vector`-shaped, or `instance` is too short to reach it.
pub fn container_len(type_name: &str, field_name: &str, instance: &[u8]) -> usize {
    ffi::reflection_container_size(type_name, field_name, instance) as usize
}

/// Reads the element at `index` of a `Vector`-shaped field, decoded as `T` via `bytemuck`. `None`
/// unless the element type is trivially copyable, `T`'s size matches the container's declared
/// element size, and `index < `[`container_len`] -- see the `sturdy-sys` reflection module doc
/// comment for why a non-trivially-copyable element type isn't supported here.
pub fn container_get_element_as<T: bytemuck::Pod>(
    type_name: &str,
    field_name: &str,
    instance: &[u8],
    index: usize,
) -> Option<T> {
    let mut out = vec![0u8; core::mem::size_of::<T>()];
    ffi::reflection_container_get_element(type_name, field_name, instance, index as u64, &mut out)
        .then(|| bytemuck::pod_read_unaligned(&out))
}

/// Writes `value` into the element at `index` of a `Vector`-shaped field. Same
/// triviality/size/range requirements as [`container_get_element_as`]; returns whether the write
/// happened.
pub fn container_set_element_value<T: bytemuck::Pod>(
    type_name: &str,
    field_name: &str,
    instance: &mut [u8],
    index: usize,
    value: T,
) -> bool {
    ffi::reflection_container_set_element(type_name, field_name, instance, index as u64, bytemuck::bytes_of(&value))
}

/// Resizes a `Vector`-shaped field to exactly `new_size` elements (default-constructing new ones,
/// destroying truncated ones). `false` when the type/field doesn't exist, the field isn't
/// `Vector`-shaped, the element type has no default constructor, or `instance` is too short.
pub fn container_resize(type_name: &str, field_name: &str, instance: &mut [u8], new_size: usize) -> bool {
    ffi::reflection_container_resize(type_name, field_name, instance, new_size as u64)
}

// -------------------------------------------------------------------------------------------
// Attributes
// -------------------------------------------------------------------------------------------

/// One `SFT::Reflection::Attribute` value -- arbitrary tooling/mod-facing metadata attached to a
/// type or field.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    Bool(bool),
    SignedInt(i64),
    Float(f64),
    String(String),
}

/// One attribute on a type or field, from [`type_attributes`]/[`field_attributes`].
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: AttributeValue,
}

impl From<ffi::AttributeSummary> for Attribute {
    fn from(value: ffi::AttributeSummary) -> Self {
        let attribute_value = match value.kind {
            ffi::AttributeKind::Bool => AttributeValue::Bool(value.bool_value),
            ffi::AttributeKind::SignedInt => AttributeValue::SignedInt(value.int_value),
            ffi::AttributeKind::Float => AttributeValue::Float(value.float_value),
            _ => AttributeValue::String(value.string_value),
        };
        Self { name: value.name, value: attribute_value }
    }
}

/// `name`'s own attributes. Empty (not an error) when `name` is not a registered type or declares
/// none.
pub fn type_attributes(name: &str) -> Vec<Attribute> {
    ffi::reflection_type_attributes(name).into_iter().map(Attribute::from).collect()
}

/// `field_name`'s own attributes on `type_name`. Empty (not an error) when the type/field doesn't
/// exist or declares none.
pub fn field_attributes(type_name: &str, field_name: &str) -> Vec<Attribute> {
    ffi::reflection_field_attributes(type_name, field_name).into_iter().map(Attribute::from).collect()
}

// -------------------------------------------------------------------------------------------
// Events
// -------------------------------------------------------------------------------------------

/// One event a type declares (`SFT::Reflection::EventInfo`) -- descriptor plus the ability to
/// [`fire_event`] it; see the `sturdy-sys` reflection module doc comment for why live
/// *subscription* still isn't attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDescriptor {
    pub name: String,
    /// Each parameter's canonical type name, in order; empty entries mean that parameter's type
    /// has no name this binding can resolve.
    pub param_type_names: Vec<String>,
}

impl From<ffi::EventSummary> for EventDescriptor {
    fn from(value: ffi::EventSummary) -> Self {
        Self { name: value.name, param_type_names: value.param_type_names }
    }
}

/// `name`'s own declared events, in declaration order. Empty (not an error) when `name` is not a
/// registered type or declares none.
pub fn events(name: &str) -> Vec<EventDescriptor> {
    ffi::reflection_type_events(name).into_iter().map(EventDescriptor::from).collect()
}

/// Looks up one event by name, rather than enumerating all of them via [`events`]. `None` when the
/// type/event doesn't exist.
pub fn find_event(type_name: &str, event_name: &str) -> Option<EventDescriptor> {
    let mut out = ffi::EventSummary::default();
    ffi::reflection_find_event(type_name, event_name, &mut out).then(|| out.into())
}

/// Fires `event_name` on `object` (an instance of `type_name`) with `args`, each entry being one
/// argument's raw bytes in [`EventDescriptor::param_type_names`] order. Calls every currently
/// subscribed listener in registration order -- a safe no-op if nobody has subscribed (see the
/// `sturdy-sys` reflection module doc comment for why *this binding* cannot itself subscribe one).
///
/// Returns `false` (firing nothing) unless `args.len()` matches the event's declared arity *and*
/// every argument's byte length matches that parameter's own registered/fundamental size exactly
/// -- this is checked engine-side before touching any of `args`' bytes, so a wrong length fails
/// closed rather than reading out of bounds.
pub fn fire_event(type_name: &str, event_name: &str, object: &mut [u8], args: &[&[u8]]) -> bool {
    let sizes: Vec<u32> = args.iter().map(|a| a.len() as u32).collect();
    let data: Vec<u8> = args.iter().flat_map(|a| a.iter().copied()).collect();
    ffi::reflection_fire_event(type_name, event_name, object, &sizes, &data)
}

// -------------------------------------------------------------------------------------------
// Method overrides, before/after hooks, and event subscription
// -------------------------------------------------------------------------------------------

use sturdy_sys::reflection::RustReflectionCallback;

/// One reflected call as seen by an override/hook/event callback, with every raw pointer the
/// engine passed already sliced to its declared size.
pub struct ReflectedCall<'a> {
    /// The receiver's bytes (the owning type's full size); `None` for a static method, or when the
    /// engine passed a null receiver.
    pub object: Option<&'a mut [u8]>,
    /// One slice per parameter, in declaration order, each exactly that parameter's size.
    pub args: Vec<&'a [u8]>,
    /// Where an override writes its return value (exactly the return type's size); `None` for
    /// `void` methods, hooks, and events.
    pub ret: Option<&'a mut [u8]>,
}

/// Why registering an override/hook/subscription failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookError {
    /// No such type, method, or event.
    NotFound,
    /// A parameter/return type's size can't be resolved (it's neither a registered type nor a
    /// fundamental), so the callback's argument pointers couldn't be sliced safely.
    UnresolvedSignature,
}

impl core::fmt::Display for HookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HookError::NotFound => write!(f, "no such type, method, or event"),
            HookError::UnresolvedSignature => write!(f, "a parameter or return type's size cannot be resolved"),
        }
    }
}

impl std::error::Error for HookError {}

/// When a [`MethodHook`] runs relative to the method body (or its override).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTiming {
    Before,
    After,
}

/// Wraps `f` into a leaked `RustReflectionCallback` that slices raw engine pointers per `layout`.
///
/// Leaked on purpose (never reclaimed, even after the registration is dropped): `Multicast` fires
/// from an RCU snapshot, so a call can still be running with this pointer after unsubscribe
/// returns. A few bytes per registration is the price of that being sound.
fn leak_callback<F>(layout: ffi::CallableLayout, f: F) -> *mut RustReflectionCallback
where
    F: Fn(&mut ReflectedCall<'_>) + Send + Sync + 'static,
{
    let object_size = layout.object_size as usize;
    let param_sizes: Vec<usize> = layout.param_sizes.iter().map(|s| *s as usize).collect();
    let return_size = layout.return_size as usize;
    let is_static = layout.is_static;
    let body = move |object: *mut u8, args: *const *const u8, out_return: *mut u8| {
        // SAFETY: the engine passes exactly `param_sizes.len()` argument pointers, each pointing at
        // a live value of that parameter's type (sized per the layout resolved at registration),
        // a receiver of the owning type (or null/unused for static methods), and a return slot of
        // the return type's size (or null).
        let mut call = unsafe {
            ReflectedCall {
                object: (!is_static && !object.is_null() && object_size > 0)
                    .then(|| core::slice::from_raw_parts_mut(object, object_size)),
                args: param_sizes
                    .iter()
                    .enumerate()
                    .map(|(i, &size)| {
                        let pointer = if args.is_null() { core::ptr::null() } else { *args.add(i) };
                        if pointer.is_null() || size == 0 { &[][..] } else { core::slice::from_raw_parts(pointer, size) }
                    })
                    .collect(),
                ret: (!out_return.is_null() && return_size > 0)
                    .then(|| core::slice::from_raw_parts_mut(out_return, return_size)),
            }
        };
        f(&mut call);
    };
    Box::into_raw(Box::new(RustReflectionCallback { body: Box::new(body) }))
}

fn checked(layout: ffi::CallableLayout, found: bool) -> Result<ffi::CallableLayout, HookError> {
    if !found {
        Err(HookError::NotFound)
    } else if !layout.ok {
        Err(HookError::UnresolvedSignature)
    } else {
        Ok(layout)
    }
}

/// Replaces `method_name`'s body on `type_name` with `f` for every reflected invocation (from Rust
/// via [`invoke_method0`], or C++ via `SFT_REFLECT_INVOKE`) until the returned handle is dropped.
/// Only one override per method; installing another replaces it. `f` writes the return value
/// into [`ReflectedCall::ret`]. Before/after hooks still run around it.
///
/// `f` may run on any thread, possibly concurrently, and must not panic (a panic aborts the
/// process -- it cannot unwind through C++).
pub fn override_method<F>(type_name: &str, method_name: &str, f: F) -> Result<MethodOverride, HookError>
where
    F: Fn(&mut ReflectedCall<'_>) + Send + Sync + 'static,
{
    let layout = ffi::reflection_method_layout(type_name, method_name);
    let found = layout.object_size != 0 || layout.ok;
    let layout = checked(layout, found)?;
    let callback = leak_callback(layout, f);
    // SAFETY: `callback` is leaked, so it outlives any possible invocation.
    if unsafe { ffi::reflection_set_method_override(type_name, method_name, callback) } {
        Ok(MethodOverride { type_name: type_name.to_owned(), method_name: method_name.to_owned() })
    } else {
        Err(HookError::NotFound)
    }
}

/// Adds a hook that runs before or after every reflected invocation of `method_name`. Any number
/// may coexist. Removed when the returned handle is dropped. Same threading/panic rules as
/// [`override_method`].
pub fn add_method_hook<F>(type_name: &str, method_name: &str, timing: HookTiming, f: F) -> Result<MethodHook, HookError>
where
    F: Fn(&mut ReflectedCall<'_>) + Send + Sync + 'static,
{
    let layout = ffi::reflection_method_layout(type_name, method_name);
    let found = layout.object_size != 0 || layout.ok;
    let layout = checked(layout, found)?;
    let after = timing == HookTiming::After;
    let callback = leak_callback(layout, f);
    // SAFETY: see `override_method`.
    let subscription = unsafe { ffi::reflection_add_method_hook(type_name, method_name, after, callback) };
    if subscription == 0 {
        return Err(HookError::NotFound);
    }
    Ok(MethodHook { type_name: type_name.to_owned(), method_name: method_name.to_owned(), after, subscription })
}

/// Subscribes `f` to `event_name` on `type_name`, called whenever the event fires (from Rust via
/// [`fire_event`], or C++ via `SFT_REFLECT_FIRE_EVENT`). Unsubscribed when the returned handle is
/// dropped. Same threading/panic rules as [`override_method`].
pub fn subscribe_event<F>(type_name: &str, event_name: &str, f: F) -> Result<EventSubscription, HookError>
where
    F: Fn(&mut ReflectedCall<'_>) + Send + Sync + 'static,
{
    let layout = ffi::reflection_event_layout(type_name, event_name);
    let found = layout.object_size != 0 || layout.ok;
    let layout = checked(layout, found)?;
    let callback = leak_callback(layout, f);
    // SAFETY: see `override_method`.
    let subscription = unsafe { ffi::reflection_subscribe_event(type_name, event_name, callback) };
    if subscription == 0 {
        return Err(HookError::NotFound);
    }
    Ok(EventSubscription { type_name: type_name.to_owned(), event_name: event_name.to_owned(), subscription })
}

/// An installed method override; clears it on drop.
#[must_use = "the override is removed as soon as this handle is dropped"]
pub struct MethodOverride {
    type_name: String,
    method_name: String,
}

impl Drop for MethodOverride {
    fn drop(&mut self) {
        ffi::reflection_clear_method_override(&self.type_name, &self.method_name);
    }
}

/// An installed before/after hook; removes it on drop.
#[must_use = "the hook is removed as soon as this handle is dropped"]
pub struct MethodHook {
    type_name: String,
    method_name: String,
    after: bool,
    subscription: u64,
}

impl Drop for MethodHook {
    fn drop(&mut self) {
        ffi::reflection_remove_method_hook(&self.type_name, &self.method_name, self.after, self.subscription);
    }
}

/// An event subscription; unsubscribes on drop.
#[must_use = "the subscription is removed as soon as this handle is dropped"]
pub struct EventSubscription {
    type_name: String,
    event_name: String,
    subscription: u64,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        ffi::reflection_unsubscribe_event(&self.type_name, &self.event_name, self.subscription);
    }
}

// -------------------------------------------------------------------------------------------
// Runtime type registration and byte-marshalled invocation
// -------------------------------------------------------------------------------------------

impl From<&Attribute> for ffi::AttributeSummary {
    fn from(value: &Attribute) -> Self {
        let mut out = ffi::AttributeSummary { name: value.name.clone(), ..Default::default() };
        match &value.value {
            AttributeValue::Bool(v) => {
                out.kind = ffi::AttributeKind::Bool;
                out.bool_value = *v;
            }
            AttributeValue::SignedInt(v) => {
                out.kind = ffi::AttributeKind::SignedInt;
                out.int_value = *v;
            }
            AttributeValue::Float(v) => {
                out.kind = ffi::AttributeKind::Float;
                out.float_value = *v;
            }
            AttributeValue::String(v) => {
                out.kind = ffi::AttributeKind::String;
                out.string_value = v.clone();
            }
        }
        out
    }
}

/// Byte size of `type_name` when it's a registered type or a fundamental (`"i32"`, `"f32"`, ...).
pub fn type_size(type_name: &str) -> Option<usize> {
    match ffi::reflection_type_size(type_name) {
        0 => None,
        size => Some(size as usize),
    }
}

/// Why [`TypeBuilder::register`] failed: the first builder or registry error, as a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRegisterError(pub String);

impl core::fmt::Display for TypeRegisterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TypeRegisterError {}

/// Registers a brand-new reflected type from Rust (`SFT::Reflection::TypeInfoBuilder` +
/// `TypeRegistry::register_type`), visible to every other reflection consumer -- C++ mods,
/// editors, serializers -- exactly like a `SFT_REFLECT_TYPE` one.
///
/// Runtime types are plain bytes: moves/copies are `memcpy`, destruction is a no-op, and default
/// construction copies the default bytes (zeroes unless [`TypeBuilder::with_default`]). Field,
/// parameter, and return types are therefore limited to fundamentals (`"i32"`, `"f32"`, `"bool"`,
/// ...) and other runtime-registered types. Method bodies are Rust closures with the same
/// threading/panic rules as [`override_method`]; overrides and hooks apply to them as usual.
///
/// Builder calls chain; the first error is reported by [`register`](Self::register).
///
/// ```ignore
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
/// struct Crate { hp: i32, weight: f32 }
///
/// TypeBuilder::for_pod::<Crate>("game.crate")
///     .field("hp", 0, "i32")
///     .field("weight", 4, "f32")
///     .method("hp_times_two", "i32", &[], |call| {
///         let hp = i32::from_ne_bytes(call.object.as_ref().unwrap()[0..4].try_into().unwrap());
///         call.ret.as_mut().unwrap().copy_from_slice(&(hp * 2).to_ne_bytes());
///     })
///     .event("on_broken", &[])
///     .register()?;
/// ```
pub struct TypeBuilder {
    inner: cxx::UniquePtr<ffi::DynamicTypeBuilder>,
    size: usize,
    error: Option<String>,
}

impl TypeBuilder {
    /// A type of `size` bytes aligned to `align`, default-constructed as zeroes.
    pub fn new(name: &str, size: usize, align: usize) -> Self {
        Self { inner: ffi::reflection_type_builder_new(name, size as u64, align as u64, &[]), size, error: None }
    }

    /// A type laid out like `T`, default-constructed as zeroes.
    pub fn for_pod<T: bytemuck::Pod>(name: &str) -> Self {
        Self::new(name, core::mem::size_of::<T>(), core::mem::align_of::<T>())
    }

    /// A type laid out like `T`, default-constructed as a copy of `default`.
    pub fn with_default<T: bytemuck::Pod>(name: &str, default: &T) -> Self {
        let size = core::mem::size_of::<T>();
        let inner =
            ffi::reflection_type_builder_new(name, size as u64, core::mem::align_of::<T>() as u64, bytemuck::bytes_of(default));
        Self { inner, size, error: None }
    }

    fn record(&mut self, error: String) {
        if self.error.is_none() && !error.is_empty() {
            self.error = Some(error);
        }
    }

    /// Declares a trivial field of fundamental or runtime-registered type `type_name` at `offset`.
    pub fn field(self, name: &str, offset: usize, type_name: &str) -> Self {
        self.field_with_attributes(name, offset, type_name, &[])
    }

    /// [`field`](Self::field) with attributes (see [`field_attributes`]).
    pub fn field_with_attributes(mut self, name: &str, offset: usize, type_name: &str, attributes: &[Attribute]) -> Self {
        let attributes: Vec<ffi::AttributeSummary> = attributes.iter().map(Into::into).collect();
        let error =
            ffi::reflection_type_builder_add_field(self.inner.pin_mut(), name, offset as u64, type_name, &attributes);
        self.record(error);
        self
    }

    /// Declares a method whose body is `f`. `return_type` is `"void"` for none. `f` receives the
    /// receiver as [`ReflectedCall::object`] (this type's full size), each argument sliced to its
    /// size, and writes the return value into [`ReflectedCall::ret`].
    pub fn method<F>(mut self, name: &str, return_type: &str, params: &[&str], f: F) -> Self
    where
        F: Fn(&mut ReflectedCall<'_>) + Send + Sync + 'static,
    {
        let mut param_sizes = Vec::with_capacity(params.len());
        for param in params {
            match type_size(param) {
                Some(size) => param_sizes.push(size as u64),
                None => {
                    self.record(format!("method `{name}`: unknown parameter type `{param}`"));
                    return self;
                }
            }
        }
        let return_size = match return_type {
            "" | "void" => 0,
            other => match type_size(other) {
                Some(size) => size as u64,
                None => {
                    self.record(format!("method `{name}`: unknown return type `{other}`"));
                    return self;
                }
            },
        };
        let layout = ffi::CallableLayout {
            ok: true,
            object_size: self.size as u64,
            param_sizes,
            return_size,
            is_static: false,
        };
        let callback = leak_callback(layout, f);
        let params: Vec<String> = params.iter().map(|p| (*p).to_owned()).collect();
        // SAFETY: `callback` is a fresh `Box::into_raw`; ownership passes to the builder, which
        // either installs it permanently or frees it when discarded.
        let error =
            unsafe { ffi::reflection_type_builder_add_method(self.inner.pin_mut(), name, return_type, &params, callback) };
        self.record(error);
        self
    }

    /// Declares an event with the given parameter types (fire with [`fire_event`], listen with
    /// [`subscribe_event`]).
    pub fn event(mut self, name: &str, params: &[&str]) -> Self {
        let params: Vec<String> = params.iter().map(|p| (*p).to_owned()).collect();
        let error = ffi::reflection_type_builder_add_event(self.inner.pin_mut(), name, &params);
        self.record(error);
        self
    }

    /// Attaches a type-level attribute (see [`type_attributes`]).
    pub fn attribute(mut self, attribute: Attribute) -> Self {
        ffi::reflection_type_builder_add_type_attribute(self.inner.pin_mut(), &(&attribute).into());
        self
    }

    /// Registers the type. Fails on the first builder error, or if the name is already taken.
    /// Dropping a builder without registering discards it.
    pub fn register(self) -> Result<(), TypeRegisterError> {
        if let Some(error) = self.error {
            return Err(TypeRegisterError(error));
        }
        match ffi::reflection_type_builder_finish(self.inner) {
            error if error.is_empty() => Ok(()),
            error => Err(TypeRegisterError(error)),
        }
    }
}

/// Unregisters `type_name` (`TypeRegistry::unregister_type`); `false` if nothing is registered
/// under it. Name lookups stop finding it, but descriptors are never freed, so code that already
/// resolved the type keeps working -- quiesce any such users before relying on it being gone.
/// Method-body closures of a runtime type are kept alive for the same reason.
pub fn unregister_type(type_name: &str) -> bool {
    ffi::reflection_unregister_type(type_name)
}

/// Invokes `method_name` with byte-marshalled `args` (one slice per parameter, each exactly that
/// parameter's size), writing the result into `ret` (exactly the return size; empty for `void`).
/// Overrides and hooks apply. Every parameter and the return type must be a fundamental or a
/// runtime-registered type -- anything else could need real C++ construction, which raw bytes
/// can't provide.
pub fn invoke_method(
    type_name: &str,
    method_name: &str,
    object: &mut [u8],
    args: &[&[u8]],
    ret: &mut [u8],
) -> Result<(), MethodInvokeError> {
    let sizes: Vec<u32> = args.iter().map(|a| a.len() as u32).collect();
    let data: Vec<u8> = args.concat();
    match ffi::reflection_invoke_method(type_name, method_name, object, &sizes, &data, ret) {
        error if error.is_empty() => Ok(()),
        error => Err(MethodInvokeError(error)),
    }
}

// -------------------------------------------------------------------------------------------
// Runtime overlay: display-name and attribute overrides
// -------------------------------------------------------------------------------------------

/// What a runtime-overlay call addresses: the type itself, or one of its own declared fields or
/// methods (by name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayTarget<'a> {
    Type,
    Field(&'a str),
    Method(&'a str),
}

fn overlay_parts<'a>(target: OverlayTarget<'a>) -> (ffi::OverlayTarget, &'a str) {
    match target {
        OverlayTarget::Type => (ffi::OverlayTarget::Type, ""),
        OverlayTarget::Field(name) => (ffi::OverlayTarget::Field, name),
        OverlayTarget::Method(name) => (ffi::OverlayTarget::Method, name),
    }
}

/// Overrides how `target` on `type_name` *presents* (to editors, script bindings, ...) without
/// touching the static descriptor or behavior: [`effective_name`] reports `name` until cleared,
/// while [`find_type`]/[`fields`]/[`methods`] keep reporting the declared names. Replaces any
/// previous override. `false` if the type/member doesn't exist.
pub fn set_name_override(type_name: &str, target: OverlayTarget<'_>, name: &str) -> bool {
    let (kind, member) = overlay_parts(target);
    ffi::reflection_overlay_set_name(kind, type_name, member, name)
}

/// Clears a [`set_name_override`]; `false` if there was none.
pub fn clear_name_override(type_name: &str, target: OverlayTarget<'_>) -> bool {
    let (kind, member) = overlay_parts(target);
    ffi::reflection_overlay_clear_name(kind, type_name, member)
}

/// Attaches `attribute` to `target`, additive to its static attributes (see
/// [`effective_attributes`]). `false` if the type/member doesn't exist.
pub fn add_attribute_override(type_name: &str, target: OverlayTarget<'_>, attribute: &Attribute) -> bool {
    let (kind, member) = overlay_parts(target);
    ffi::reflection_overlay_add_attribute(kind, type_name, member, &attribute.into())
}

/// Removes every overlay (name and attributes) on `target`; `false` if there was none.
pub fn clear_overlay(type_name: &str, target: OverlayTarget<'_>) -> bool {
    let (kind, member) = overlay_parts(target);
    ffi::reflection_overlay_clear(kind, type_name, member)
}

/// `target`'s effective name: the override if set, else its declared name. `None` if the
/// type/member doesn't exist.
pub fn effective_name(type_name: &str, target: OverlayTarget<'_>) -> Option<String> {
    let (kind, member) = overlay_parts(target);
    let mut out = String::new();
    ffi::reflection_overlay_effective_name(kind, type_name, member, &mut out).then_some(out)
}

/// `target`'s effective attributes: its static attributes, then any overlay ones. Empty if the
/// type/member doesn't exist.
pub fn effective_attributes(type_name: &str, target: OverlayTarget<'_>) -> Vec<Attribute> {
    let (kind, member) = overlay_parts(target);
    ffi::reflection_overlay_effective_attributes(kind, type_name, member).into_iter().map(Attribute::from).collect()
}
