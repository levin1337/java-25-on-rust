use crate::share::oops::fieldInfo::FieldInfo;

pub struct FieldDescriptor<'a> {
    pub field_info: FieldInfo<'a>,
}

impl<'a> FieldDescriptor<'a> {
    pub fn new(field_info: FieldInfo<'a>) -> Self {
        Self { field_info }
    }

    pub fn name(&self) -> &'a [u8] {
        self.field_info.name
    }

    pub fn signature(&self) -> &'a [u8] {
        self.field_info.descriptor
    }

    pub fn is_static(&self) -> bool {
        self.field_info.is_static()
    }

    pub fn slot(&self) -> usize {
        self.field_info.slot
    }
}
