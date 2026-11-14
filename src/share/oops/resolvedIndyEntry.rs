/*
 * Port of OpenJDK 25 HotSpot ResolvedIndyEntry (share/oops/resolvedIndyEntry.hpp)
 *
 * ResolvedIndyEntry holds the resolution information for invokedynamic bytecodes (0xba),
 * caching the resolved CallSite and MethodHandle target.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIndyEntry {
    cpool_index: u16,
    bsm_index: u16,
    method_name: Option<String>,
    signature: Option<String>,
    call_site_target: Option<String>,
    flags: u16,
}

impl ResolvedIndyEntry {
    pub const FLAG_IS_RESOLVED: u16 = 1 << 0;

    pub fn new(cpool_index: u16, bsm_index: u16) -> Self {
        Self {
            cpool_index,
            bsm_index,
            method_name: None,
            signature: None,
            call_site_target: None,
            flags: 0,
        }
    }

    #[inline]
    pub fn cpool_index(&self) -> u16 {
        self.cpool_index
    }

    #[inline]
    pub fn bsm_index(&self) -> u16 {
        self.bsm_index
    }

    #[inline]
    pub fn is_resolved(&self) -> bool {
        (self.flags & Self::FLAG_IS_RESOLVED) != 0
    }

    #[inline]
    pub fn method_name(&self) -> Option<&str> {
        self.method_name.as_deref()
    }

    #[inline]
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    #[inline]
    pub fn call_site_target(&self) -> Option<&str> {
        self.call_site_target.as_deref()
    }

    pub fn fill_in(&mut self, method_name: String, signature: String, target: String) {
        self.method_name = Some(method_name);
        self.signature = Some(signature);
        self.call_site_target = Some(target);
        self.flags |= Self::FLAG_IS_RESOLVED;
    }
}
