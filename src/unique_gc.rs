#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

use core::{
    alloc::Layout,
    borrow::Borrow,
    fmt::{self, Debug, Display, Pointer},
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use crate::{
    meta_sized::MetaSized,
    types::{GcBoxHeader, GcBoxInner, Invariant},
    Collect, Gc, Mutation,
};

/// A uniquely-owned garbage-collected pointer to a type `T`.
///
/// Unlike [`Gc`] this pointer is known to be unique,
/// and as such allows mutation without the use of interior mutability. It does not however,
/// implement [`Collect`], and as such is intended for the initialisation of data, before
/// converting into a plain [`Gc`] with [`UniqueGc::into_gc`].
///
/// [`Gc`]: crate::Gc
/// [`Collect`]: crate::Collect
pub struct UniqueGc<'gc, T: ?Sized + 'gc> {
    pub(crate) ptr: NonNull<GcBoxInner<T>>,
    pub(crate) _invariant: Invariant<'gc>,
}

impl<'gc, T: Debug + ?Sized + 'gc> Debug for UniqueGc<'gc, T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, fmt)
    }
}

impl<'gc, T: ?Sized + 'gc> Pointer for UniqueGc<'gc, T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&UniqueGc::as_ptr(self), fmt)
    }
}

impl<'gc, T: Display + ?Sized + 'gc> Display for UniqueGc<'gc, T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, fmt)
    }
}

impl<'gc, T: ?Sized + 'gc> Deref for UniqueGc<'gc, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: Since `self` is immutably borrowed, and by `UniqueGc`'s
        // invariants, we can soundly materialise an immutable reference to
        // `value`.
        unsafe { &self.ptr.as_ref().value }
    }
}

impl<'gc, T: ?Sized + 'gc> DerefMut for UniqueGc<'gc, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Since `self` is mutably borrowed, and by `UniqueGc`'s
        // invariants, we can soundly materialise a mutable reference to
        // `value`.
        unsafe { &mut self.ptr.as_mut().value }
    }
}

impl<'gc, T: ?Sized + 'gc> AsRef<T> for UniqueGc<'gc, T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<'gc, T: ?Sized + 'gc> AsMut<T> for UniqueGc<'gc, T> {
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<'gc, T: ?Sized + 'gc> Borrow<T> for UniqueGc<'gc, T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<'gc, T: Collect<'gc>> UniqueGc<'gc, T> {
    /// Creates a new `UniqueGc` containing the given value.
    ///
    /// As the allocated value is statically known not to have any other references, we can safely
    /// modify it through this pointer.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let mut gc = UniqueGc::new(mc, 42i32);
    ///
    /// assert_eq!(*gc, 42);
    /// *gc = 0;
    /// assert_eq!(*gc, 0);
    /// # });
    /// ```
    pub fn new(mc: &Mutation<'gc>, val: T) -> UniqueGc<'gc, T> {
        let gc = UniqueGc::new_uninit(mc);

        gc.write(val)
    }

    /// Creates a new `UniqueGc` with uninitialized contents.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let mut gc = UniqueGc::<i32>::new_uninit(mc);
    ///
    /// // let gc = unsafe { gc.assume_init() };
    /// //                      ^ undefined behaviour
    ///
    /// (*gc).write(42);
    /// let gc = unsafe { gc.assume_init() };
    /// assert_eq!(*gc, 42);
    /// # })
    /// ```
    pub fn new_uninit(mc: &Mutation<'gc>) -> UniqueGc<'gc, MaybeUninit<T>> {
        // SAFETY: `allocate_metasized` is unsafe because the value is uninitialized. This is what
        // we want for this function.
        let ptr = unsafe { mc.allocate_metasized::<MaybeUninit<T>>(()) };

        UniqueGc {
            ptr,
            _invariant: PhantomData,
        }
    }

    /// Creates a new `UniqueGc` with uninitialized contents, with the memory being filled with `0` bytes.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let gc = UniqueGc::<i32>::new_zeroed(mc);
    ///
    /// // SAFETY: It is valid to initialize an `i32` with zeroed bytes.
    /// let gc = unsafe { gc.assume_init() };
    ///
    /// assert_eq!(*gc, 0);
    /// # });
    /// ```
    pub fn new_zeroed(mc: &Mutation<'gc>) -> UniqueGc<'gc, MaybeUninit<T>> {
        let mut gc = UniqueGc::<T>::new_uninit(mc);

        // SAFETY: Value is stored in a `MaybeUninit`.
        unsafe { gc.as_mut_ptr().write_bytes(0x00, 1) };

        gc
    }
}

impl<'gc, T: Collect<'gc>> UniqueGc<'gc, [T]> {
    /// Constructs a new garbage-collected slice with uninitialized contents.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let mut values = UniqueGc::<[i32]>::new_uninit_slice(mc, 3);
    ///
    /// let values = unsafe {
    ///     // Deferred initialization:
    ///     values[0].as_mut_ptr().write(1);
    ///     values[1].as_mut_ptr().write(2);
    ///     values[2].as_mut_ptr().write(3);
    ///
    ///     values.assume_init()
    /// };
    ///
    /// assert_eq!(*values, [1, 2, 3]);
    /// # });
    /// ```
    pub fn new_uninit_slice(mc: &Mutation<'gc>, len: usize) -> UniqueGc<'gc, [MaybeUninit<T>]> {
        // SAFETY: `[MaybeUninit<_>]` may be entirely unintialized.
        let ptr = unsafe { mc.allocate_metasized::<[MaybeUninit<T>]>(len) };

        UniqueGc {
            ptr,
            _invariant: PhantomData,
        }
    }

    /// Constructs a new garbage-collected slice with unitialized contents, with the memory being filled with `0` bytes.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let values = UniqueGc::<[i32]>::new_zeroed_slice(mc, 3);
    /// let values = unsafe { values.assume_init() };
    ///
    /// assert_eq!(*values, [0, 0, 0]);
    /// # });
    /// ```
    pub fn new_zeroed_slice(mc: &Mutation<'gc>, len: usize) -> UniqueGc<'gc, [MaybeUninit<T>]> {
        let mut gc = UniqueGc::new_uninit_slice(mc, len);

        // SAFETY: `[MaybeUninit<T>]` may be safely set to zero bytes.
        unsafe { gc.as_mut_ptr().write_bytes(0x00, len) };

        gc
    }

    /// Constructs a new garbage-collected slice, copying each element from the given slice.
    ///
    /// If `T` does not implement `Copy`, use [`clone_from_slice`].
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let src = [1, 2, 3, 4];
    ///
    /// let gc = UniqueGc::copy_from_slice(mc, &src[1..3]);
    ///
    /// assert_eq!(src, [1, 2, 3, 4]);
    /// assert_eq!(*gc, [2, 3]);
    /// # });
    /// ```
    pub fn copy_from_slice(mc: &Mutation<'gc>, s: &[T]) -> UniqueGc<'gc, [T]>
    where
        T: Copy,
    {
        let mut gc = UniqueGc::new_uninit_slice(mc, s.len());

        // SAFETY:
        // - `s` and `gc` cannot be overlapping as `gc` was freshly allocated.
        // - `s` is valid for reads of `s.len() * size_of::<T>()` bytes.
        // - `gc` is allocated with length `s.len()` and so is valid for writes
        //   of `s.len() * size_of::<T>()` bytes.
        // - This cannot cause double drops, as `T: Copy`.
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), gc.as_mut_ptr().cast::<T>(), s.len()) };

        // SAFETY: `gc` was initialized above.
        unsafe { gc.assume_init() }
    }

    /// Constructs a new garbage-collected slice, cloning each element from the given slice.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// use std::rc::Rc;
    ///
    /// let src = [Rc::new(1), Rc::new(2), Rc::new(3), Rc::new(4)];
    ///
    /// let gc = UniqueGc::clone_from_slice(mc, &src[1..3]);
    ///
    /// assert_eq!(src.map(|rc| Rc::strong_count(&rc)), [1, 2, 2, 1]);
    /// # });
    /// ```
    pub fn clone_from_slice(mc: &Mutation<'gc>, s: &[T]) -> UniqueGc<'gc, [T]>
    where
        T: Clone,
    {
        let mut gc = UniqueGc::new_uninit_slice(mc, s.len());

        for (place, val) in gc.iter_mut().zip(s) {
            place.write(val.clone());
        }

        // SAFETY: `gc` was initialized above.
        unsafe { gc.assume_init() }
    }

    /// Constructs a new garbage-collected slice, taking ownership of each element in the given vector.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let v = vec![1, 2, 3, 4];
    ///
    /// let gc = UniqueGc::from_vec(mc, v);
    ///
    /// assert_eq!(*gc, [1, 2, 3, 4]);
    /// # });
    /// ```
    pub fn from_vec(mc: &Mutation<'gc>, mut s: alloc::vec::Vec<T>) -> UniqueGc<'gc, [T]> {
        let mut gc = UniqueGc::new_uninit_slice(mc, s.len());

        // SAFETY:
        // - `gc` and `s` cannot overlap, since `gc` is freshly allocated.
        // - The values contained in `s` will be forgotten by the following
        //   call to `s.set_len()`, and as such will not be dropped twice.
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), gc.as_mut_ptr().cast::<T>(), s.len());
        }

        // SAFETY:
        // - `0` is trivially always less than or equal to `capacity()`.
        // - `old_len..new_len` is always empty, so no elements need to be
        //   initialized.
        unsafe { s.set_len(0) };

        // SAFETY: Every element in `gc` has been initialized by the prior call
        // to `copy_nonoverlapping`.
        unsafe { gc.assume_init() }
    }
}

impl<'gc, T: Collect<'gc>> UniqueGc<'gc, MaybeUninit<T>> {
    /// Converts to `UniqueGc<'gc, T>`.
    ///
    /// # Safety
    /// As with [`MaybeUninit::assume_init`], it is up to the caller to guarantee
    /// that the value really is in an initialized state. Calling this when the
    /// content is not yet fully initialized will likely cause undefined behaviour.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let mut gc = UniqueGc::<i32>::new_uninit(mc);
    ///
    /// let gc: UniqueGc<'_, i32> = unsafe {
    ///     gc.as_mut_ptr().write(42);
    ///
    ///     gc.assume_init()
    /// };
    ///
    /// assert_eq!(*gc, 42);
    /// # });
    /// ```
    pub unsafe fn assume_init(self) -> UniqueGc<'gc, T> {
        // SAFETY: `MaybeUninit<T>` is guaranteed to have the same layout as `T`.
        unsafe { self.transmute() }
    }

    /// Writes the value and converts to `UniqueGc<'gc, T>`
    ///
    /// This method converts the pointer similarly to [`UniqueGc::assume_init`]
    /// but writes `value` into it before conversion, thus guaranteeing safety.
    pub fn write(mut self, value: T) -> UniqueGc<'gc, T> {
        (*self).write(value);

        // SAFETY: The contained value was just initialized.
        unsafe { self.assume_init() }
    }
}

impl<'gc, T: Collect<'gc>> UniqueGc<'gc, [MaybeUninit<T>]> {
    /// Converts to `UniqueGc<'gc, [T]>`.
    ///
    /// # Safety
    /// As with [`MaybeUninit::assume_init`], it is up to the caller to
    /// guarantee that the values really are in an initialized state. Calling
    /// this when the contents are not yet fully initialized will likely cause
    /// undefined behaviour.
    ///
    /// # Examples
    /// ```
    /// # use gc_arena::{arena::rootless_mutate, UniqueGc};
    /// # rootless_mutate(|mc| {
    /// let mut values = UniqueGc::<[i32]>::new_uninit_slice(mc, 3);
    ///
    /// let values = unsafe {
    ///     // Deferred initialization:
    ///     values[0].as_mut_ptr().write(1);
    ///     values[1].as_mut_ptr().write(2);
    ///     values[2].as_mut_ptr().write(3);
    ///
    ///     values.assume_init()
    /// };
    ///
    /// assert_eq!(*values, [1, 2, 3]);
    /// # });
    /// ```
    pub unsafe fn assume_init(self) -> UniqueGc<'gc, [T]> {
        // SAFETY: `[MaybeUninit<T>]` is guaranteed to have the same layout as `[T]`.
        unsafe { self.transmute() }
    }
}

impl<'gc, T: ?Sized + 'gc> UniqueGc<'gc, T> {
    /// Returns a raw mutable pointer to the `UniqueGc`'s contents.
    ///
    /// Very few guarantees are given about this pointer, except that it is properly
    /// aligned, points to a valid instance of `T`, and may be written to.
    pub fn as_mut_ptr(this: &mut UniqueGc<'gc, T>) -> *mut T {
        // SAFETY: `UniqueGc` is guaranteed to contain a pointer to a valid instance of a `GcBoxInner<T>`.
        unsafe {
            let inner = this.ptr.as_ptr();
            (&raw mut (*inner).value) as *mut T
        }
    }

    /// Returns a raw pointer to the `UniqueGc`'s contents.
    ///
    /// Very few guarantees are given about this pointer, except that it is properly
    /// aligned, and points to a valid instance of `T`
    pub fn as_ptr(this: &UniqueGc<'gc, T>) -> *const T {
        // SAFETY: `UniqueGc` is guaranteed to contain a pointer to a valid instance of a `GcBoxInner<T>`.
        unsafe {
            let inner = this.ptr.as_ptr();
            (&raw const (*inner).value) as *mut T
        }
    }

    /// Constructs a `UniqueGc` from a raw pointer.
    ///
    /// # Safety
    /// The given pointer must have been obtained from [`UniqueGc::as_ptr`] or
    /// [`Gc::as_ptr`]. There must also exist no other garbage collected pointers
    /// which point to the same allocation. This is always the case for [`UniqueGc::as_ptr`].
    pub unsafe fn from_raw(raw: *mut T) -> UniqueGc<'gc, T> {
        let layout = Layout::new::<GcBoxHeader>();
        // SAFETY: A precondition of this function is that `raw` must point to
        // the `value` field of a valid `GcBoxInner`.
        let (_, header_offset) = layout.extend(Layout::for_value(unsafe { &*raw })).unwrap();
        let header_offset = -(header_offset as isize);
        // SAFETY: The given pointer must point to the `value` field of a valid
        // `GcBoxInner`, and `header_offset` is the number of bytes between the
        // `value` field, and the start of a `GcBoxInner<T>`.
        let ptr = unsafe { raw.byte_offset(header_offset) as *mut GcBoxInner<T> };
        UniqueGc {
            // SAFETY: Function precondition.
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            _invariant: PhantomData,
        }
    }

    /// Converts the `UniqueGc` into a regular [`Gc`].
    pub fn into_gc(this: UniqueGc<'gc, T>) -> Gc<'gc, T> {
        // SAFETY: Trivial.
        unsafe { Gc::from_ptr(UniqueGc::as_ptr(&this)) }
    }
}

impl<'gc, T: ?Sized + MetaSized> UniqueGc<'gc, T> {
    /// Very unsafely transmutes the given `UniqueGc` to have a different type.
    ///
    /// This is not a shallow pointer based transmutation, this changes the v-table as well.
    ///
    /// # Safety
    /// - A pointer to `T` must point to a value with the same size and alignment as that same
    ///   pointer bitwise interpreted to a pointer to `U`.
    pub(crate) unsafe fn transmute<U: ?Sized + MetaSized<Metadata = T::Metadata> + Collect<'gc>>(
        self,
    ) -> UniqueGc<'gc, U> {
        let (ptr, meta) = <GcBoxInner<T>>::box_parts_mut(self.ptr.as_ptr());

        let u_ptr = <GcBoxInner<U>>::from_box_parts_mut(ptr, meta);

        // SAFETY: The safety of this call is a precondition of this function.
        unsafe {
            (*u_ptr.cast::<GcBoxHeader>()).set_vtable::<U>();
        }

        UniqueGc {
            // SAFETY: `u_ptr` is the same pointer which came from `self`, and so must not be null.
            ptr: unsafe { NonNull::new_unchecked(u_ptr) },
            _invariant: PhantomData,
        }
    }
}

#[cfg(test)]
mod test {
    use std::{rc::Rc, vec::Vec};

    use super::*;
    use crate::{arena::rootless_mutate, Collect};

    #[test]
    fn unique_gc_drops() {
        use std::{cell::Cell, thread_local};
        thread_local! {
            static DROPPED: Cell<bool> = const { Cell::new(false) };
        }

        struct DropWatcher;

        impl Drop for DropWatcher {
            fn drop(&mut self) {
                DROPPED.set(true);
            }
        }

        // SAFETY: DropWatcher's drop implementation does not dereference any garbage-collected pointers.
        unsafe impl Collect<'_> for DropWatcher {
            const NEEDS_TRACE: bool = false;
        }

        rootless_mutate(|mc| {
            UniqueGc::new(mc, DropWatcher);
        });

        assert!(DROPPED.get());
    }

    #[test]
    fn unique_gc_new() {
        rootless_mutate(|mc| {
            let mut gc = UniqueGc::new(mc, 12i32);
            assert_eq!(*gc, 12);
            *gc = 42;
            assert_eq!(*gc, 42);
        });
    }

    #[test]
    fn unique_gc_uninit() {
        rootless_mutate(|mc| {
            let gc = UniqueGc::new_uninit(mc);
            let gc1 = gc.write(0);
            assert_eq!(*gc1, 0);

            // SAFETY: `i32` can be safely zero-initialized.
            let gc2 = unsafe { UniqueGc::<i32>::new_zeroed(mc).assume_init() };
            assert_eq!(*gc1, *gc2);
        });
    }

    #[test]
    fn unique_gc_from_vec() {
        let rc = Rc::new(());
        rootless_mutate(|mc| {
            let v = Vec::from([rc.clone()]);

            assert_eq!(Rc::strong_count(&rc), 2);

            let _gc = UniqueGc::from_vec(mc, v);

            assert_eq!(Rc::strong_count(&rc), 2);
        });
        assert_eq!(Rc::strong_count(&rc), 1);
    }
}
