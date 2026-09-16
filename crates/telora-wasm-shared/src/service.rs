//! Fixed compiler-to-runtime service contract. All function identities are
//! indirect table slots assigned by the linker; all layouts are sealed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Contract {
    pub initialize: u32,
    pub entry: u32,
    pub materialize: u32,
    pub names_offset: u32,
    pub initializer_offset: u32,
    pub context_type: u32,
    pub dict_type: u32,
    pub value_bytes: u32,
    pub sources_offset: u32,
}

impl Contract {
    pub fn words(self) -> [u32; 9] {
        [self.initialize, self.entry, self.materialize, self.names_offset,
            self.initializer_offset, self.context_type, self.dict_type,
            self.value_bytes, self.sources_offset]
    }
}
