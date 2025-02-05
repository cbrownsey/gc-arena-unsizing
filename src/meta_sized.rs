use core::alloc::{Layout, LayoutError};

use sealed::Sealed;

mod sealed {
    pub trait Sealed {}

    impl<T> Sealed for T {}
    impl<T> Sealed for [T] {}
    impl Sealed for str {}
}

pub trait MetaSized: Sealed {
    type Metadata: Copy + Sized;

    fn layout_from_meta(meta: Self::Metadata) -> Result<Layout, LayoutError>;

    fn into_parts(this: *const Self) -> (*const (), Self::Metadata);
    fn from_parts(ptr: *const (), meta: Self::Metadata) -> *const Self;
    fn into_parts_mut(this: *mut Self) -> (*mut (), Self::Metadata);
    fn from_parts_mut(ptr: *mut (), meta: Self::Metadata) -> *mut Self;
}

impl<T> MetaSized for T {
    type Metadata = ();

    fn layout_from_meta(_: ()) -> Result<Layout, LayoutError> {
        Ok(Layout::new::<T>())
    }

    fn into_parts(this: *const Self) -> (*const (), ()) {
        (this.cast::<()>(), ())
    }

    fn from_parts(ptr: *const (), _meta: ()) -> *const T {
        ptr.cast::<T>()
    }

    fn into_parts_mut(this: *mut Self) -> (*mut (), ()) {
        (this.cast::<()>(), ())
    }

    fn from_parts_mut(ptr: *mut (), _meta: ()) -> *mut T {
        ptr.cast::<T>()
    }
}

impl<T> MetaSized for [T] {
    type Metadata = usize;

    fn layout_from_meta(len: usize) -> Result<Layout, LayoutError> {
        Layout::array::<T>(len)
    }

    fn into_parts(this: *const Self) -> (*const (), usize) {
        (this.cast::<()>(), this.len())
    }

    fn from_parts(ptr: *const (), len: usize) -> *const Self {
        core::ptr::slice_from_raw_parts(ptr.cast::<T>(), len)
    }

    fn into_parts_mut(this: *mut Self) -> (*mut (), usize) {
        (this.cast::<()>(), this.len())
    }

    fn from_parts_mut(ptr: *mut (), len: usize) -> *mut Self {
        core::ptr::slice_from_raw_parts_mut(ptr.cast::<T>(), len)
    }
}

impl MetaSized for str {
    type Metadata = usize;

    fn layout_from_meta(len: usize) -> Result<Layout, LayoutError> {
        <[u8] as MetaSized>::layout_from_meta(len)
    }

    fn into_parts(this: *const Self) -> (*const (), usize) {
        <[u8] as MetaSized>::into_parts(this as *const [u8])
    }

    fn from_parts(ptr: *const (), len: usize) -> *const Self {
        <[u8] as MetaSized>::from_parts(ptr, len) as *const str
    }

    fn into_parts_mut(this: *mut Self) -> (*mut (), usize) {
        <[u8] as MetaSized>::into_parts_mut(this as *mut [u8])
    }

    fn from_parts_mut(ptr: *mut (), len: usize) -> *mut Self {
        <[u8] as MetaSized>::from_parts_mut(ptr, len) as *mut str
    }
}
