use crate::{CallContext, NativeError};

pub(crate) fn native_get(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    context.set_type_property_option(context.result(), context.argument(1)?, context.argument(0)?)
}
