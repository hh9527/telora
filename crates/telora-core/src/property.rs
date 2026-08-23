use crate::{CallContext, NativeError};

pub(crate) fn native_get_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    context.set_type_property_option(context.result(), context.argument(0)?, context.argument(1)?)
}

pub(crate) fn native_get_field(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    context.set_field_property_option(
        context.result(),
        context.argument(0)?,
        context.argument(1)?,
        context.argument(2)?,
    )
}

pub(crate) fn native_get_variant(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    context.set_variant_property_option(
        context.result(),
        context.argument(0)?,
        context.argument(1)?,
        context.argument(2)?,
    )
}
