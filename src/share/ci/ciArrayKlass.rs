/*
 * Port of OpenJDK 25 HotSpot ciArrayKlass (share/ci/ciArrayKlass.hpp/.cpp)
 *
 * ciArrayKlass is the ciType/ciKlass view of an array class.  Array
 * klasses have no InstanceKlass backing in the VM metadata store, so the
 * facade stores the element ciType and the array dimension directly.
 */

use super::ciType::CIType;

/// `ciArrayKlass`: compiler-interface view of `[T`, `[[T`, ...
///
/// `element` is the ultimate (non-array) element type and `dimension`
/// the total number of array dimensions, mirroring `ArrayKlass::dimension`
/// and `ArrayKlass::element_klass` semantics.
#[derive(Clone, PartialEq, Eq)]
pub struct CiArrayKlass<'a> {
    // Boxed: CIType::Array contains CiArrayKlass by value, so the element
    // type must be behind a pointer to keep the enum sized.
    element: Box<CIType<'a>>,
    dimension: u32,
}

impl<'a> CiArrayKlass<'a> {
    pub fn new(element: CIType<'a>, dimension: u32) -> Self {
        Self {
            element: Box::new(element),
            dimension,
        }
    }

    /// The type of `array[i]` one level down: for `[[I` this is `[I`,
    /// for `[I` it is `int`.
    pub fn element_klass(&self) -> CIType<'a> {
        if self.dimension > 1 {
            CIType::Array(CiArrayKlass::new(
                (*self.element).clone(),
                self.dimension - 1,
            ))
        } else {
            (*self.element).clone()
        }
    }

    /// Ultimate non-array element type (`ArrayKlass::element_klass`
    /// bottoming out at the base type).
    #[inline]
    pub fn element_type(&self) -> &CIType<'a> {
        self.element.as_ref()
    }

    /// `ArrayKlass::dimension`.
    #[inline]
    pub fn dimension(&self) -> u32 {
        self.dimension
    }

    /// JVM descriptor of this array type: `[I`, `[[Ljava/lang/String;`, ...
    pub fn descriptor(&self) -> String {
        let mut d = String::with_capacity(self.dimension as usize + 16);
        for _ in 0..self.dimension {
            d.push('[');
        }
        d.push_str(&self.element.descriptor());
        d
    }

    /// Whether this is a primitive array (`[I`, `[J`, ...).
    pub fn is_primitive_array(&self) -> bool {
        self.element.is_primitive()
    }

    /// Whether this is an array of references (`[Ljava/lang/Object;`).
    pub fn is_reference_array(&self) -> bool {
        !self.is_primitive_array()
    }

    /// `ciArrayKlass::is_type` analogue: same element type and dimension.
    pub fn is_same_type(&self, other: &CiArrayKlass<'a>) -> bool {
        self.dimension == other.dimension && self.element == other.element
    }
}

impl<'a> std::fmt::Debug for CiArrayKlass<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.descriptor())
    }
}
