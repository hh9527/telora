//! Generated shadow frames. No Wasm local is inspected without a published role.
use alloc::{collections::BTreeMap, vec::Vec};
use telora_wasm_shared::{
    abi::*,
    gc_roots::{Frame, Handle, Kind, Root, Roots},
};

static mut PINS: BTreeMap<u64, Handle> = BTreeMap::new();
static mut ROOTS: Roots = Roots::new(1 << 24);
static mut FRAMES: Vec<Frame> = Vec::new();
static mut AUDIT_INTERVAL: u32 = 0;
static mut NEXT_AUDIT: u32 = 0;
static mut MARKS: u32 = 0;
static mut GC_INTERVAL: u32 = 64 * 1024;
static mut NEXT_GC: u64 = 64 * 1024;
static mut COLLECTIONS: u32 = 0;

unsafe fn roots() -> &'static mut Roots {
    unsafe { &mut *core::ptr::addr_of_mut!(ROOTS) }
}
unsafe fn frames() -> &'static mut Vec<Frame> {
    unsafe { &mut *core::ptr::addr_of_mut!(FRAMES) }
}

pub(crate) unsafe fn snapshot() -> Vec<Root> {
    unsafe { roots().iter().collect() }
}

/// Collect the exact graph at a published execution boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_collect() -> u32 {
    unsafe {
        let mark = crate::gc_mark::Mark::trace(snapshot());
        crate::tables::sweep(&mark.objects);
        crate::sources::sweep(&mark.sources);
        crate::heap::sweep(mark.storage);
        COLLECTIONS = COLLECTIONS.checked_add(1).unwrap();
    }
    0
}

/// Set the allocation interval; zero disables automatic collection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_interval(bytes: u32) -> u32 {
    unsafe {
        GC_INTERVAL = bytes;
        NEXT_GC = crate::heap::allocation_clock().checked_add(u64::from(bytes)).unwrap();
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_collections() -> u32 {
    unsafe { *core::ptr::addr_of!(COLLECTIONS) }
}

pub(crate) unsafe fn reset_frames() {
    unsafe {
        while let Some(frame) = frames().pop() {
            roots().leave(frame).expect("reset root frame");
        }
        roots().invalidate_pins();
        (&mut *core::ptr::addr_of_mut!(PINS)).clear();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_enter(count: u32) -> u32 {
    unsafe {
        frames().try_reserve(1).expect("root frame handles");
        let frame = roots().enter(count as usize).expect("root frame entry");
        frames().push(frame);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_leave() -> u32 {
    unsafe {
        let frame = frames().pop().expect("root frame underflow");
        roots().leave(frame).expect("root frame exit");
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_set(
    slot: u32,
    reference: u32,
    role: u32,
    ty: u32,
    initialized: u32,
) -> u32 {
    let kind = match role {
        ROOT_VALUE => Kind::Value { type_id: ty },
        ROOT_STORAGE => Kind::Storage,
        ROOT_PARSE_PLAN => Kind::ParsePlan,
        ROOT_PARSE_CONTEXT | ROOT_CODEC_CONTEXT => Kind::Context {
            string_type: ty,
            codec: role == ROOT_CODEC_CONTEXT,
        },
        ROOT_SOURCE => Kind::Source,
        ROOT_REFERENCES => Kind::References {
            type_id: ty,
            initialized,
        },
        tag if tag < TABLE_COUNT
            || matches!(tag, RECORDS | ARRAYS | VALUES | NEWTYPES | ENVIRONMENTS) =>
        {
            Kind::Object {
                type_id: ty,
                tag,
                initialized,
            }
        }
        _ => panic!("invalid root role"),
    };
    unsafe {
        let frame = *frames().last().expect("root outside frame");

        roots()
            .set(frame, slot as usize, Some(Root { reference, kind }))
            .expect("root publication");
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_audit(interval: u32) -> u32 {
    unsafe {
        AUDIT_INTERVAL = interval;
        NEXT_AUDIT = 0;
        MARKS = 0;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_safepoint() -> u32 {
    unsafe {
        let gc_interval = *core::ptr::addr_of!(GC_INTERVAL);
        if gc_interval != 0 && (gc_interval == 1 || crate::heap::allocation_clock() >= *core::ptr::addr_of!(NEXT_GC)) {
            telora_gc_collect();
            NEXT_GC = crate::heap::allocation_clock()
                .checked_add(crate::heap::collection_interval(gc_interval))
                .unwrap();
        }
        let interval = *core::ptr::addr_of!(AUDIT_INTERVAL);
        if interval != 0 {
            let bytes = crate::heap::telora_heap_bytes();
            if bytes >= *core::ptr::addr_of!(NEXT_AUDIT) {
                crate::gc_mark::Mark::trace(snapshot());
                NEXT_AUDIT = bytes.saturating_add(interval);
                MARKS = MARKS.checked_add(1).unwrap();
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_depth() -> u32 {
    unsafe { roots().depth() as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_count() -> u32 {
    unsafe { roots().iter().count() as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_roots_marks() -> u32 {
    unsafe { *core::ptr::addr_of!(MARKS) }
}

pub(crate) unsafe fn collect_pins(collector: &mut crate::collect::Collector) {
    unsafe {
        roots().rewrite_pins(|root| {
            let Kind::Value { type_id } = root.kind else {
                unreachable!("Host pin role");
            };
            Root {
                reference: collector.value(root.reference, type_id),
                ..root
            }
        });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_pin(reference: u32, ty: u32) -> u64 {
    unsafe {
        if reference == 0 {
            return 0;
        }
        let Ok(handle) = roots().pin(Root {
            reference,
            kind: Kind::Value { type_id: ty },
        }) else {
            return 0;
        };
        let token = handle.generation();
        assert!(
            (&mut *core::ptr::addr_of_mut!(PINS))
                .insert(token, handle)
                .is_none()
        );
        token
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_pinned(token: u64) -> u32 {
    unsafe {
        (&*core::ptr::addr_of!(PINS))
            .get(&token)
            .and_then(|handle| roots().get(*handle).ok())
            .map_or(0, |root| root.reference)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_unpin(token: u64) -> u32 {
    unsafe {
        let Some(handle) = (&mut *core::ptr::addr_of_mut!(PINS)).remove(&token) else {
            return 0;
        };
        roots().unpin(handle).map_or(0, |root| root.reference)
    }
}

/// Host acknowledges owned feedback only between language invocations. Existing
/// baseline bytes remain immutable so service reset can restore its snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_release_feedback(active: u32) -> u32 {
    unsafe {
        assert!(frames().is_empty(), "feedback release during execution");
        for tag in [DIAGNOSTICS, DEBUG_EVENTS] {
            crate::heap::write(table_address(tag), crate::tables::Table {
                buffer: 0, length: 0, capacity: 0, reserved: 0,
            });
        }
        if active != 0 {
            crate::tables::telora_table_push(table_address(DIAGNOSTICS), active, DIAGNOSTIC_BYTES);
        }
    }
    0
}
