use core::alloc::{Layout, LayoutError};
use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::{mem, ptr};

use crate::meta_sized::MetaSized;
use crate::{collect::Collect, context::Context};

/// A thin-pointer-sized box containing a type-erased GC object.
/// Stores the metadata required by the GC algorithm inline (see `GcBoxInner`
/// for its typed counterpart).

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct GcBox(NonNull<GcBoxInner<()>>);

impl GcBox {
    /// Erases a pointer to a typed GC object.
    ///
    /// **SAFETY:** The pointer must point to a valid `GcBoxInner` allocated
    /// in a `Box`.
    #[inline(always)]
    pub(crate) unsafe fn erase<T: ?Sized>(ptr: NonNull<GcBoxInner<T>>) -> Self {
        // This cast is sound because `GcBoxInner` is `repr(C)`.
        let erased = ptr.as_ptr() as *mut GcBoxInner<()>;
        Self(NonNull::new_unchecked(erased))
    }

    // / Gets a pointer to the value stored inside this box.
    // / `T` must be the same type that was used with `erase`, so that
    // / we can correctly compute the field offset.
    // #[deprecated]
    // #[inline(always)]
    // fn unerased_value<T>(&self) -> *mut T {
    //     unsafe {
    //         let ptr = self.0.as_ptr() as *mut GcBoxInner<T>;
    //         // Don't create a reference, to keep the full provenance.
    //         // Also, this gives us interior mutability "for free".
    //         ptr::addr_of_mut!((*ptr).value) as *mut T
    //     }
    // }

    /// Gets a pointer to the value stored inside this `GcBox`. The returned
    /// pointer will have the metadata of the originally allocated object. If
    /// an alternative metadata is required, use [`typed_ptr_with`].
    ///
    /// **SAFETY:** The value stored in the `GcBox` must be readable as the type `T`, and
    /// the metadata associated with that value must be readable as, and have
    /// the same size and alignment as `T::Metadata`.
    unsafe fn typed_ptr<T: ?Sized + MetaSized>(&self) -> *mut T {
        self.typed_ptr_with(self.typed_metadata::<T>())
    }

    /// Gets a pointer to the value stored inside this `GcBox`, with the given
    /// metadata.
    ///
    /// **SAFETY**:
    /// - The alignment of the value stored in the `GcBox` must be equal to the alignment of `T`.
    /// - The size of the value stored in the `GcBox` must be less than or equal to than the size
    ///   of `T`.
    unsafe fn typed_ptr_with<T: ?Sized + MetaSized>(&self, meta: T::Metadata) -> *mut T {
        // this is sound, since the metadata of an unsized struct is identical to the metadata of its unsized field.
        let ptr = T::from_parts_mut(self.0.as_ptr().cast::<()>(), meta) as *mut GcBoxInner<T, ()>;

        // Safety: A `GcBox` must always point to the second field of a valid
        // `GcBoxInner<T, T::Metadata>`. This necessarily means that it also points to the first
        // field of a valid `GcBoxInner<T , ()>`.
        (unsafe { &raw mut (*ptr).value }) as *mut T
    }

    /// Copies the metadata associated with this `GcBox`.
    ///
    /// **SAFETY:** The metadata value stored in the `GcBox` must have the exact same layout as `T`.
    unsafe fn typed_metadata<T: ?Sized + MetaSized>(&self) -> T::Metadata {
        // Safety: A `GcBox` must always point to the second field of a valid `GcBoxInner<T, T::Metadata>`.
        unsafe {
            self.0
                .as_ptr()
                .byte_sub(GcBoxInner::<T>::metadata_offset().unwrap())
                .cast::<T::Metadata>()
                .read()
        }
    }

    #[inline(always)]
    pub(crate) fn header(&self) -> &GcBoxHeader {
        unsafe { &self.0.as_ref().header }
    }

    /// Traces the stored value.
    ///
    /// **SAFETY**: `Self::drop_in_place` must not have been called.
    #[inline(always)]
    pub(crate) unsafe fn trace_value(&self, cc: &mut Context) {
        (self.header().vtable().trace_value)(*self, cc)
    }

    /// Drops the stored value.
    ///
    /// **SAFETY**: once called, no GC pointers should access the stored value
    /// (but accessing the `GcBox` itself is still safe).
    #[inline(always)]
    pub(crate) unsafe fn drop_in_place(&mut self) {
        (self.header().vtable().drop_value)(*self)
    }

    /// Deallocates the box. Failing to call `Self::drop_in_place` beforehand
    /// will cause the stored value to be leaked.
    ///
    /// **SAFETY**: once called, this `GcBox` should never be accessed by any GC
    /// pointers again.
    #[inline(always)]
    pub(crate) unsafe fn dealloc(self) {
        // let layout = self.header().vtable().box_layout_old;
        let (layout, offset) = (self.header().vtable().box_layout)(self);

        let ptr = (self.0.as_ptr() as *mut u8).byte_sub(offset);
        // SAFETY: the pointer was `Box`-allocated with this layout.
        alloc::alloc::dealloc(ptr, layout);
    }

    pub(crate) fn size_of_box(&self) -> usize {
        unsafe { (self.header().vtable().box_layout)(*self) }
            .0
            .size()
    }
}

pub(crate) struct GcBoxHeader {
    /// The next element in the global linked list of allocated objects.
    next: Cell<Option<GcBox>>,
    /// A custom virtual function table for handling type-specific operations.
    ///
    /// The lower bits of the pointer are used to store GC flags:
    /// - bits 0 & 1 for the current `GcColor`;
    /// - bit 2 for the `needs_trace` flag;
    /// - bit 3 for the `is_live` flag.
    tagged_vtable: Cell<*const CollectVtable>,
}

impl GcBoxHeader {
    #[inline(always)]
    pub fn new<'gc, T: ?Sized + MetaSized + Collect<'gc>>() -> Self {
        // Helper trait to materialize vtables in static memory.
        // trait HasCollectVtable {
        //     const VTABLE: CollectVtable;
        // }

        // impl<'gc, T: Collect<'gc>> HasCollectVtable for T {
        //     const VTABLE: CollectVtable = CollectVtable::vtable_for::<T>();
        // }

        // let vtable: &'static _ = &<T as HasCollectVtable>::VTABLE;

        let vtable: &'static _ = CollectVtable::vtable_for::<T>();
        Self {
            next: Cell::new(None),
            tagged_vtable: Cell::new(vtable as *const _),
        }
    }

    /// Gets a reference to the `CollectVtable` used by this box.
    #[inline(always)]
    fn vtable(&self) -> &'static CollectVtable {
        let ptr = tagged_ptr::untag(self.tagged_vtable.get());
        // SAFETY:
        // - the pointer was properly untagged.
        // - the vtable is stored in static memory.
        unsafe { &*ptr }
    }

    /// Gets the next element in the global linked list of allocated objects.
    #[inline(always)]
    pub(crate) fn next(&self) -> Option<GcBox> {
        self.next.get()
    }

    /// Sets the next element in the global linked list of allocated objects.
    #[inline(always)]
    pub(crate) fn set_next(&self, next: Option<GcBox>) {
        self.next.set(next)
    }

    // / Returns the (shallow) size occupied by this box in memory.
    // #[inline(always)]
    // #[deprecated]
    // pub(crate) fn size_of_box(&self) -> usize {
    //     panic!()
    // }

    #[inline]
    pub(crate) fn color(&self) -> GcColor {
        match tagged_ptr::get::<0x3, _>(self.tagged_vtable.get()) {
            0x0 => GcColor::White,
            0x1 => GcColor::WhiteWeak,
            0x2 => GcColor::Gray,
            _ => GcColor::Black,
        }
    }

    #[inline]
    pub(crate) fn set_color(&self, color: GcColor) {
        tagged_ptr::set::<0x3, _>(
            &self.tagged_vtable,
            match color {
                GcColor::White => 0x0,
                GcColor::WhiteWeak => 0x1,
                GcColor::Gray => 0x2,
                GcColor::Black => 0x3,
            },
        );
    }
    #[inline]
    pub(crate) fn needs_trace(&self) -> bool {
        tagged_ptr::get::<0x4, _>(self.tagged_vtable.get()) != 0x0
    }

    /// Determines whether or not we've dropped the `dyn Collect` value
    /// stored in `GcBox.value`
    /// When we garbage-collect a `GcBox` that still has outstanding weak pointers,
    /// we set `alive` to false. When there are no more weak pointers remaining,
    /// we will deallocate the `GcBox`, but skip dropping the `dyn Collect` value
    /// (since we've already done it).
    #[inline]
    pub(crate) fn is_live(&self) -> bool {
        tagged_ptr::get::<0x8, _>(self.tagged_vtable.get()) != 0x0
    }

    #[inline]
    pub(crate) fn set_needs_trace(&self, needs_trace: bool) {
        tagged_ptr::set_bool::<0x4, _>(&self.tagged_vtable, needs_trace);
    }

    #[inline]
    pub(crate) fn set_live(&self, alive: bool) {
        tagged_ptr::set_bool::<0x8, _>(&self.tagged_vtable, alive);
    }

    /// Sets the vtable stored in this `GcBox` to the vtable for the given type.
    ///
    /// # Safety
    /// The new vtable must be valid for the new type. This means that a pointer
    #[inline]
    pub(crate) unsafe fn set_vtable<'gc, T: ?Sized + MetaSized + Collect<'gc>>(&self) {
        let tag = tagged_ptr::get::<0xF, _>(self.tagged_vtable.get());
        let new_vtable = CollectVtable::vtable_for::<T>();

        let tagged = tagged_ptr::tagged::<0xF, _>(new_vtable, tag);

        self.tagged_vtable.set(tagged);
    }
}

/// Type-specific operations for GC'd values.
///
/// We use a custom vtable instead of `dyn Collect` for extra flexibility.
/// The type is over-aligned so that `GcBoxHeader` can store flags into the LSBs of the vtable pointer.
#[repr(align(16))]
struct CollectVtable {
    /// Computes the layout of the `GcBox` the GC'd value is stored in and the offset, in bytes,
    /// from the start of the metadata field to the start of the `GcBoxHeader`.
    box_layout: unsafe fn(GcBox) -> (Layout, usize),
    /// Drops the value stored in the given `GcBox` (without deallocating the box).
    drop_value: unsafe fn(GcBox),
    /// Traces the value stored in the given `GcBox`.
    trace_value: unsafe fn(GcBox, &mut Context),
}

impl CollectVtable {
    // / Makes a vtable for a known, `Sized` type.
    // / Because `T: Sized`, we can recover a typed pointer
    // / directly from the erased `GcBox`.
    // #[inline(always)]
    // #[deprecated]
    // const fn vtable_for<'gc, T: Collect<'gc>>() -> Self {
    //     Self {
    //         box_layout_old: Layout::new::<GcBoxInner<T>>(),
    //         box_layout: |_| Layout::new::<T>(),
    //         drop_value: |erased| unsafe {
    //             ptr::drop_in_place(erased.unerased_value::<T>());
    //         },
    //         trace_value: |erased, cc| unsafe {
    //             let val = &*(erased.unerased_value::<T>());
    //             val.trace(cc)
    //         },
    //     }
    // }

    /// Makes a vtable for a known, `Unsized` type.
    /// Because `T: MetaSized`, we can recover a typed pointer
    /// directly from the erased `GcBox`.
    const fn vtable_for<'gc, T: ?Sized + MetaSized + Collect<'gc>>() -> &'static Self {
        &Self {
            box_layout: |erased| {
                let meta = unsafe { erased.typed_metadata::<T>() };

                let Ok((layout, offset)) = (|| -> Result<(Layout, usize), LayoutError> {
                    let layout = GcBoxInner::<T>::box_layout(meta)?;
                    let offset = GcBoxInner::<T>::metadata_offset()?;

                    Ok((layout, offset))
                })() else {
                    panic!()
                };

                (layout, offset)
            },
            drop_value: |erased| unsafe {
                ptr::drop_in_place(erased.typed_ptr::<T>());
            },
            trace_value: |erased, cc| unsafe {
                let val = &*(erased.typed_ptr::<T>());
                val.trace(cc)
            },
        }
    }
}

/// A typed GC'd value, together with its metadata.
/// This type is never manipulated directly by the GC algorithm, allowing
/// user-facing `Gc`s to freely cast their pointer to it.
#[repr(C)]
pub(crate) struct GcBoxInner<T: ?Sized, M = ()> {
    /// The metadata associated with the value stored in this `GcBox`. This
    /// field is usually never read or written, except when the `GcBox` is
    /// being created or dropped.
    pub(crate) metadata: M,
    pub(crate) header: GcBoxHeader,
    /// The typed value stored in this `GcBox`.
    pub(crate) value: mem::ManuallyDrop<T>,
}

impl<'gc, T: Collect<'gc>> GcBoxInner<T> {
    #[inline(always)]
    pub(crate) fn new(header: GcBoxHeader, t: T) -> Self {
        Self {
            metadata: (),
            header,
            value: mem::ManuallyDrop::new(t),
        }
    }
}

impl<T: ?Sized + MetaSized> GcBoxInner<T> {
    pub(crate) fn from_box_parts_mut(ptr: *mut (), meta: T::Metadata) -> *mut GcBoxInner<T> {
        T::from_parts_mut(ptr, meta) as *mut GcBoxInner<T>
    }

    pub(crate) fn box_parts_mut(this: *mut GcBoxInner<T>) -> (*mut (), T::Metadata) {
        T::into_parts_mut(this as *mut T)
    }

    pub(crate) fn metadata_offset() -> Result<usize, LayoutError> {
        let layout = Layout::new::<T::Metadata>();
        let (_, offset) = layout.extend(Layout::new::<GcBoxHeader>())?;
        Ok(offset)
    }

    pub(crate) fn box_layout(meta: T::Metadata) -> Result<Layout, LayoutError> {
        let layout = Layout::new::<T::Metadata>();
        let (layout, _) = layout.extend(Layout::new::<GcBoxHeader>())?;
        let (layout, _) = layout.extend(T::layout_from_meta(meta)?)?;
        Ok(layout.pad_to_align())
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum GcColor {
    /// An object that has not yet been reached by tracing (if we're in a tracing phase).
    ///
    /// During `Phase::Sweep`, we will free all white objects that existed *before* the start of the
    /// current `Phase::Sweep`. Objects allocated during `Phase::Sweep` will be white, but will not
    /// be freed.
    White,
    /// Like White, but for objects weakly reachable from a Black object.
    ///
    /// These objects may drop their contents during `Phase::Sweep`, but must stay allocated so that
    /// weak references can check the alive status.
    WhiteWeak,
    /// An object reachable from a Black object, but that has not yet been traced using
    /// `Collect::trace`. We also mark black objects as gray during `Phase::Mark` in response to
    /// a write barrier, so that we re-trace and find any objects newly reachable from the mutated
    /// object.
    Gray,
    /// An object that was reached during tracing. It will not be freed during `Phase::Sweep`. At
    /// the end of `Phase::Sweep`, all black objects will be reset to white.
    Black,
}

// Phantom type that holds a lifetime and ensures that it is invariant.
pub(crate) type Invariant<'a> = PhantomData<Cell<&'a ()>>;

/// Utility functions for tagging and untagging pointers.
mod tagged_ptr {
    #![cfg_attr(not(miri), allow(unstable_name_collisions))]

    #[cfg(not(miri))]
    use sptr::Strict as _;

    use core::cell::Cell;

    trait ValidMask<const MASK: usize> {
        const CHECK: ();
    }

    impl<T, const MASK: usize> ValidMask<MASK> for T {
        const CHECK: () = assert!(MASK < core::mem::align_of::<T>());
    }

    /// Checks that `$mask` can be used to tag a pointer to `$type`.
    /// If this isn't true, this macro will cause a post-monomorphization error.
    macro_rules! check_mask {
        ($type:ty, $mask:expr) => {
            let _ = <$type as ValidMask<$mask>>::CHECK;
        };
    }

    pub(super) fn tagged<const MASK: usize, T>(ptr: *const T, tag: usize) -> *const T {
        check_mask!(T, MASK);
        ptr.map_addr(|addr| (addr & !MASK) | (tag & MASK))
    }

    #[inline(always)]
    pub(super) fn untag<T>(tagged_ptr: *const T) -> *const T {
        let mask = core::mem::align_of::<T>() - 1;
        tagged_ptr.map_addr(|addr| addr & !mask)
    }

    #[inline(always)]
    pub(super) fn get<const MASK: usize, T>(tagged_ptr: *const T) -> usize {
        check_mask!(T, MASK);
        tagged_ptr.addr() & MASK
    }

    #[inline(always)]
    pub(super) fn set<const MASK: usize, T>(pcell: &Cell<*const T>, tag: usize) {
        check_mask!(T, MASK);
        let ptr = pcell.get();
        let ptr = ptr.map_addr(|addr| (addr & !MASK) | (tag & MASK));
        pcell.set(ptr)
    }

    #[inline(always)]
    pub(super) fn set_bool<const MASK: usize, T>(pcell: &Cell<*const T>, value: bool) {
        check_mask!(T, MASK);
        let ptr = pcell.get();
        let ptr = ptr.map_addr(|addr| (addr & !MASK) | if value { MASK } else { 0 });
        pcell.set(ptr)
    }
}
