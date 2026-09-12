use super::*;
use crate::abi::Origin;

#[derive(Clone)]
pub(super) struct Blame {
    pub message: Box<[u64]>,
    pub subjects: Vec<Origin>,
}
impl Runtime {
    pub(crate) fn blame(&mut self, ty: TypeId, loc: Location, message: &Value, subjects: Vec<Origin>) -> Result<Value> {
        self.expect(ty, Kind::Blame)?;
        self.text(message.as_ref())?;
        let id = HeapRef::new(World::Work, u32::try_from(self.work.blames.len()).map_err(|_| "Blame table overflow")?)?;
        self.work.blames.push(Blame { message: message.words().into(), subjects });
        self.pack(ty, loc, &[u64::from(id.raw())])
    }
    pub(super) fn blame_object(&self, value: &Value) -> Result<&Blame> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Blame)?;
        let id = HeapRef::from_raw(u32::try_from(value.words()[2]).map_err(|_| "invalid Blame HeapId")?);
        self.tables(id).blames.get(id.slot() as usize).ok_or("invalid Blame HeapId".into())
    }
    pub(crate) fn blame_diagnostic(&self, value: &Value) -> Result<(String, Vec<Origin>)> {
        let blame = self.blame_object(value)?;
        let message = ValueRef { arena: self.identity, words: &blame.message };
        Ok((self.text(message)?.as_str().to_owned(), blame.subjects.clone()))
    }
}
