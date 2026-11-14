/*
 * Port of OpenJDK 25 HotSpot ciType (share/ci/ciType.hpp)
 *
 * Defines compiler interface representation of Java types.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CIType {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Void,
    Object(String),
    Array(Box<CIType>),
}

impl CIType {
    pub fn parse_single_descriptor(desc: &str) -> Option<(CIType, usize)> {
        let mut chars = desc.char_indices();
        let (_, first) = chars.next()?;
        match first {
            'Z' => Some((CIType::Boolean, 1)),
            'B' => Some((CIType::Byte, 1)),
            'C' => Some((CIType::Char, 1)),
            'S' => Some((CIType::Short, 1)),
            'I' => Some((CIType::Int, 1)),
            'J' => Some((CIType::Long, 1)),
            'F' => Some((CIType::Float, 1)),
            'D' => Some((CIType::Double, 1)),
            'V' => Some((CIType::Void, 1)),
            '[' => {
                let (inner, inner_len) = Self::parse_single_descriptor(&desc[1..])?;
                Some((CIType::Array(Box::new(inner)), 1 + inner_len))
            }
            'L' => {
                let end = desc.find(';')?;
                let class_name = &desc[1..end];
                Some((CIType::Object(class_name.to_string()), end + 1))
            }
            _ => None,
        }
    }

    pub fn parse_signature(sig: &str) -> Result<(Vec<CIType>, CIType), &'static str> {
        if !sig.starts_with('(') {
            return Err("Invalid signature: missing '('");
        }
        let close = sig.find(')').ok_or("Invalid signature: missing ')'")?;
        let mut params = Vec::new();
        let mut offset = 1;
        while offset < close {
            let (param_type, len) = Self::parse_single_descriptor(&sig[offset..])
                .ok_or("Invalid parameter descriptor in signature")?;
            params.push(param_type);
            offset += len;
        }
        let (ret_type, _) = Self::parse_single_descriptor(&sig[close + 1..])
            .ok_or("Invalid return type descriptor in signature")?;
        Ok((params, ret_type))
    }

    pub fn from_descriptor(desc: &str) -> Self {
        if let Some((t, len)) = Self::parse_single_descriptor(desc) {
            if len == desc.len() {
                return t;
            }
        }
        // Fallback for bare class names (e.g. "java/lang/String")
        Self::Object(desc.to_string())
    }

    pub fn descriptor(&self) -> String {
        match self {
            Self::Boolean => "Z".to_string(),
            Self::Byte => "B".to_string(),
            Self::Char => "C".to_string(),
            Self::Short => "S".to_string(),
            Self::Int => "I".to_string(),
            Self::Long => "J".to_string(),
            Self::Float => "F".to_string(),
            Self::Double => "D".to_string(),
            Self::Void => "V".to_string(),
            Self::Object(name) => format!("L{};", name),
            Self::Array(elem) => format!("[{}", elem.descriptor()),
        }
    }

    #[inline]
    pub fn is_primitive(&self) -> bool {
        !matches!(self, Self::Object(_) | Self::Array(_))
    }

    #[inline]
    pub fn is_reference(&self) -> bool {
        matches!(self, Self::Object(_) | Self::Array(_))
    }

    #[inline]
    pub fn is_void(&self) -> bool {
        matches!(self, Self::Void)
    }

    #[inline]
    pub fn size(&self) -> usize {
        match self {
            Self::Long | Self::Double => 2,
            Self::Void => 0,
            _ => 1,
        }
    }

    #[inline]
    pub fn is_one_word(&self) -> bool {
        self.size() == 1
    }

    #[inline]
    pub fn is_two_word(&self) -> bool {
        self.size() == 2
    }
}
