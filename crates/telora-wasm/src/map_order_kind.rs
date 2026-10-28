use crate::emit::Emitter;
use telora_core::mir::TypeConstructor as T;
impl Emitter<'_> {
    pub(crate) fn map_order_kind(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let kind = match self.mir.types[args[0].index()].constructor {
            T::TypeOf => {
                let k = self.mir.types[args[0].index()].arguments[0];
                match self.mir.types[k.index()].constructor {
                    T::Int | T::Bool | T::Type | T::TypeOf => 1,
                    T::String => 2,
                    _ => 0,
                }
            }
            _ => return Err("Map key order expects TypeOf(K)".into()),
        };
        self.scalar_as(self.key.node, *args.last().unwrap(), kind)
    }
}
