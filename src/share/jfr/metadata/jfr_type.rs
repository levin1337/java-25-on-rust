/*
 * OpenJDK 25 HotSpot jfr/metadata/jfrType.hpp + jfrSerializer.hpp port
 *
 * JfrType — metadata type descriptors (structs/primitives/annotations)
 * with their field lists, mirroring the generated jfrTypes.hpp model.
 */

use crate::share::jfr::utilities::jfr_types::JfrTypeId;

#[derive(Clone, Debug)]
pub struct JfrFieldDescriptor {
    pub name: &'static str,
    pub type_id: JfrTypeId,
    pub constant_pool: bool,
    pub content_type: &'static str,
    pub field_flags: u32,
}

#[derive(Clone, Debug)]
pub struct JfrTypeDescriptor {
    pub name: &'static str,
    pub type_id: JfrTypeId,
    pub supertype_id: Option<JfrTypeId>,
    pub is_struct: bool,
    pub is_instant_event: bool,
    pub fields: &'static [JfrFieldDescriptor],
    pub settings: &'static [JfrSettingDescriptorRef],
}

#[derive(Clone, Debug)]
pub struct JfrSettingDescriptorRef {
    pub name: &'static str,
    pub type_name: &'static str,
    pub default_value: &'static str,
    pub content_type: &'static str,
}

// Primitive/annotation type descriptors for the core type ids.
pub fn primitive_type(id: JfrTypeId, name: &'static str) -> JfrTypeDescriptor {
    JfrTypeDescriptor {
        name,
        type_id: id,
        supertype_id: None,
        is_struct: false,
        is_instant_event: false,
        fields: &[],
        settings: &[],
    }
}

pub fn is_known_type(id: JfrTypeId) -> bool {
    id.as_u64() >= JfrTypeId::VirtualSpace.as_u64()
        && id.as_u64() <= JfrTypeId::String.as_u64()
}
