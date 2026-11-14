/*
 * Port of OpenJDK 25 HotSpot ResolvedFieldEntry (share/oops/resolvedFieldEntry.hpp)
 *
 * ResolvedFieldEntry contains resolution information for field access bytecodes
 * (getfield, putfield, getstatic, putstatic).
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFieldEntry {
    cpool_index: u16,
    field_holder: Option<String>,
    field_offset: u32,
    field_index: u16,
    tos_state: u8,
    flags: u16,
    get_code: u8,
    put_code: u8,
}

impl ResolvedFieldEntry {
    pub const FLAG_IS_VOLATILE: u16 = 1 << 0;
    pub const FLAG_IS_FINAL: u16 = 1 << 1;
    pub const FLAG_IS_STATIC: u16 = 1 << 2;

    pub fn new(cpool_index: u16) -> Self {
        Self {
            cpool_index,
            field_holder: None,
            field_offset: 0,
            field_index: 0,
            tos_state: 0,
            flags: 0,
            get_code: 0,
            put_code: 0,
        }
    }

    #[inline]
    pub fn cpool_index(&self) -> u16 {
        self.cpool_index
    }

    #[inline]
    pub fn is_resolved(&self, is_put: bool) -> bool {
        if is_put {
            self.put_code != 0
        } else {
            self.get_code != 0
        }
    }

    #[inline]
    pub fn field_holder(&self) -> Option<&str> {
        self.field_holder.as_deref()
    }

    #[inline]
    pub fn field_offset(&self) -> u32 {
        self.field_offset
    }

    #[inline]
    pub fn field_index(&self) -> u16 {
        self.field_index
    }

    #[inline]
    pub fn tos_state(&self) -> u8 {
        self.tos_state
    }

    #[inline]
    pub fn flags(&self) -> u16 {
        self.flags
    }

    #[inline]
    pub fn is_volatile(&self) -> bool {
        (self.flags & Self::FLAG_IS_VOLATILE) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.flags & Self::FLAG_IS_FINAL) != 0
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        (self.flags & Self::FLAG_IS_STATIC) != 0
    }

    #[inline]
    pub fn get_code(&self) -> u8 {
        self.get_code
    }

    #[inline]
    pub fn put_code(&self) -> u8 {
        self.put_code
    }

    pub fn fill_in(
        &mut self,
        field_holder: String,
        field_offset: u32,
        field_index: u16,
        tos_state: u8,
        flags: u16,
        get_code: u8,
        put_code: u8,
    ) {
        self.field_holder = Some(field_holder);
        self.field_offset = field_offset;
        self.field_index = field_index;
        self.tos_state = tos_state;
        self.flags = flags;
        self.get_code = get_code;
        self.put_code = put_code;
    }
}
