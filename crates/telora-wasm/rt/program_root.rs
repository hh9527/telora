//! A typed executable root, independent of the Service protocol.
static mut ROOT: Option<(u32, u32)> = None;
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_program_root(ty: u32, value: u32) {
    unsafe {
        ROOT = Some((ty, value));
    }
}
pub(crate) unsafe fn collect(gc: &mut crate::collect::Collector) {
    unsafe {
        if let Some((ty, value)) = ROOT {
            ROOT = Some((ty, gc.value(value, ty)));
        } else {
            crate::service::collect_initialization(gc);
        }
    }
}
pub(crate) unsafe fn collect_ready(gc: &mut crate::collect::Collector) {
    unsafe {
        if matches!(ROOT, Some(_)) {
            collect(gc);
        } else {
            crate::service::collect_ready(gc);
        }
    }
}
pub(crate) unsafe fn snapshot() -> u32 {
    unsafe {
        match ROOT {
            Some((_, value)) => value,
            None => crate::service::snapshot(),
        }
    }
}
pub(crate) unsafe fn replace(value: u32) {
    unsafe {
        if let Some((ty, _)) = ROOT {
            ROOT = Some((ty, value));
        } else {
            crate::service::replace_handler(value);
        }
    }
}
pub(crate) unsafe fn restore(value: u32) {
    unsafe {
        if matches!(ROOT, Some(_)) {
            replace(value);
        } else {
            crate::service::restore(value);
        }
    }
}

pub(crate) unsafe fn execution_roots() -> alloc::vec::Vec<(u32, u32)> {
    unsafe {
        let mut roots = crate::service::execution_roots();
        if let Some((ty, value)) = ROOT {
            if value != 0 {
                roots.push((value, ty));
            }
        }
        roots
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_program_entry() -> u32 {
    unsafe { ROOT.expect("missing staged program root").1 }
}
