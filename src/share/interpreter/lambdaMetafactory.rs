/*
 * LambdaMetafactory support for invokedynamic.
 *
 * Instead of running the java.lang.invoke machinery (MethodHandles, LambdaForms,
 * hidden classes), we generate an equivalent implementation class directly as
 * a parsed ClassFile structure and define it through the ClassLoader. The
 * generated class implements the SAM interface; its fields hold the captured
 * call-site arguments and its SAM method forwards to the impl method named by
 * the bootstrap arguments, with descriptor adaptation (checkcast / box / unbox).
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::classFileParser::{Attribute, ClassFile, Constant, Member};
use crate::share::interpreter::bootstrapInfo::{BootstrapArg, BootstrapInfo};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::io::{self, Error};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static LAMBDA_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A JVM type: either a primitive (single-byte descriptor tag) or a reference
/// stored as its field descriptor ("Ljava/lang/String;" or "[I").
#[derive(Clone, Debug, PartialEq)]
enum Type {
    Prim(u8),
    Ref(String),
}

impl Type {
    fn parse_at(bytes: &[u8], i: &mut usize) -> Option<Type> {
        if *i >= bytes.len() {
            return None;
        }
        match bytes[*i] {
            b'B' | b'Z' | b'C' | b'S' | b'I' | b'F' | b'J' | b'D' | b'V' => {
                let tag = bytes[*i];
                *i += 1;
                Some(Type::Prim(tag))
            }
            b'L' => {
                let start = *i;
                *i += 1;
                while *i < bytes.len() && bytes[*i] != b';' {
                    *i += 1;
                }
                if *i >= bytes.len() {
                    return None;
                }
                *i += 1;
                Some(Type::Ref(
                    String::from_utf8_lossy(&bytes[start..*i]).into_owned(),
                ))
            }
            b'[' => {
                let start = *i;
                while *i < bytes.len() {
                    match bytes[*i] {
                        b'[' => *i += 1,
                        b'L' => {
                            *i += 1;
                            while *i < bytes.len() && bytes[*i] != b';' {
                                *i += 1;
                            }
                            if *i >= bytes.len() {
                                return None;
                            }
                            *i += 1;
                            break;
                        }
                        b'B' | b'Z' | b'C' | b'S' | b'I' | b'F' | b'J' | b'D' => {
                            *i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                Some(Type::Ref(
                    String::from_utf8_lossy(&bytes[start..*i]).into_owned(),
                ))
            }
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn parse_many(desc: &str) -> Option<Vec<Type>> {
        let bytes = desc.as_bytes();
        let mut types = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            types.push(Self::parse_at(bytes, &mut i)?);
        }
        Some(types)
    }

    /// Parses "(params)return".
    fn method(desc: &str) -> Option<(Vec<Type>, Type)> {
        let bytes = desc.as_bytes();
        if bytes.first() != Some(&b'(') {
            return None;
        }
        let close = bytes.iter().position(|b| *b == b')')?;
        let mut i = 1;
        let mut params = Vec::new();
        while i < close {
            params.push(Self::parse_at(bytes, &mut i)?);
        }
        let mut j = close + 1;
        let ret = Self::parse_at(bytes, &mut j)?;
        if j != bytes.len() {
            return None;
        }
        Some((params, ret))
    }

    fn descriptor(&self) -> String {
        match self {
            Type::Prim(tag) => (*tag as char).to_string(),
            Type::Ref(desc) => desc.clone(),
        }
    }

    /// Internal class name used by checkcast / class constants: strips the
    /// "L...;" wrapper for plain classes, keeps "[...]" for arrays.
    fn internal_name(&self) -> Option<String> {
        match self {
            Type::Prim(_) => None,
            Type::Ref(desc) => {
                if let Some(inner) = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')) {
                    Some(inner.to_owned())
                } else {
                    Some(desc.clone())
                }
            }
        }
    }

    fn slots(&self) -> usize {
        match self {
            Type::Prim(b'J') | Type::Prim(b'D') => 2,
            _ => 1,
        }
    }

    fn is_void(&self) -> bool {
        matches!(self, Type::Prim(b'V'))
    }
}

struct ImplMethod {
    kind: u8,
    owner: String,
    name: String,
    descriptor: String,
}

/// Minimal constant pool builder. Indices line up with the `constants` vector
/// (index 0 is `Unusable`, matching ClassFileParser output).
struct Cp<'a> {
    entries: Vec<Constant<'a>>,
}

impl<'a> Cp<'a> {
    fn new() -> Self {
        Self {
            entries: vec![Constant::Unusable],
        }
    }

    fn utf8(&mut self, loader: &ClassLoader<'a>, text: &str) -> u16 {
        let index = self.entries.len() as u16;
        self.entries
            .push(Constant::Utf8(loader.storage.alloc_str(text).as_bytes()));
        index
    }

    fn class(&mut self, loader: &ClassLoader<'a>, internal: &str) -> u16 {
        let name = self.utf8(loader, internal);
        let index = self.entries.len() as u16;
        self.entries.push(Constant::Class(name));
        index
    }

    fn name_and_type(&mut self, loader: &ClassLoader<'a>, name: &str, desc: &str) -> u16 {
        let n = self.utf8(loader, name);
        let d = self.utf8(loader, desc);
        let index = self.entries.len() as u16;
        self.entries.push(Constant::NameAndType(n, d));
        index
    }

    fn method_ref(
        &mut self,
        loader: &ClassLoader<'a>,
        owner: &str,
        name: &str,
        desc: &str,
        interface: bool,
    ) -> u16 {
        let c = self.class(loader, owner);
        let nt = self.name_and_type(loader, name, desc);
        let index = self.entries.len() as u16;
        self.entries.push(if interface {
            Constant::InterfaceMethodRef(c, nt)
        } else {
            Constant::MethodRef(c, nt)
        });
        index
    }

    fn field_ref(&mut self, loader: &ClassLoader<'a>, owner: &str, name: &str, desc: &str) -> u16 {
        let c = self.class(loader, owner);
        let nt = self.name_and_type(loader, name, desc);
        let index = self.entries.len() as u16;
        self.entries.push(Constant::FieldRef(c, nt));
        index
    }
}

fn emit_u16(code: &mut Vec<u8>, value: u16) {
    code.extend_from_slice(&value.to_be_bytes());
}

/// Emits a typed local-variable load (`iload`/`lload`/`fload`/`dload`/`aload`)
/// for the given slot index, with a `wide` prefix when the index exceeds 255.
fn emit_load(code: &mut Vec<u8>, index: usize, ty: &Type) {
    let op = match ty {
        Type::Prim(b'J') => 0x16,
        Type::Prim(b'F') => 0x17,
        Type::Prim(b'D') => 0x18,
        Type::Prim(b'I' | b'Z' | b'B' | b'C' | b'S') => 0x15,
        _ => 0x19,
    };
    if index < 256 {
        code.push(op);
        code.push(index as u8);
    } else {
        code.push(0xc4); // wide
        code.push(op);
        emit_u16(code, index as u16);
    }
}

fn code_attribute<'a>(
    loader: &ClassLoader<'a>,
    max_stack: usize,
    max_locals: usize,
    code: Vec<u8>,
) -> Attribute<'a> {
    let mut bytes = Vec::with_capacity(code.len() + 12);
    bytes.extend_from_slice(&(max_stack as u16).to_be_bytes());
    bytes.extend_from_slice(&(max_locals as u16).to_be_bytes());
    bytes.extend_from_slice(&(code.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&code);
    bytes.extend_from_slice(&0u16.to_be_bytes()); // exception_table_length
    Attribute {
        name: b"Code",
        bytes: loader.storage.alloc_slice_copy(&bytes),
    }
}

fn wrapper_unbox(tag: u8) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match tag {
        b'Z' => ("java/lang/Boolean", "booleanValue", "()Z"),
        b'B' => ("java/lang/Byte", "byteValue", "()B"),
        b'C' => ("java/lang/Character", "charValue", "()C"),
        b'S' => ("java/lang/Short", "shortValue", "()S"),
        b'I' => ("java/lang/Integer", "intValue", "()I"),
        b'J' => ("java/lang/Long", "longValue", "()J"),
        b'F' => ("java/lang/Float", "floatValue", "()F"),
        b'D' => ("java/lang/Double", "doubleValue", "()D"),
        _ => return None,
    })
}

fn wrapper_box(tag: u8) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match tag {
        b'Z' => ("java/lang/Boolean", "valueOf", "Z", "Ljava/lang/Boolean;"),
        b'B' => ("java/lang/Byte", "valueOf", "B", "Ljava/lang/Byte;"),
        b'C' => ("java/lang/Character", "valueOf", "C", "Ljava/lang/Character;"),
        b'S' => ("java/lang/Short", "valueOf", "S", "Ljava/lang/Short;"),
        b'I' => ("java/lang/Integer", "valueOf", "I", "Ljava/lang/Integer;"),
        b'J' => ("java/lang/Long", "valueOf", "J", "Ljava/lang/Long;"),
        b'F' => ("java/lang/Float", "valueOf", "F", "Ljava/lang/Float;"),
        b'D' => ("java/lang/Double", "valueOf", "D", "Ljava/lang/Double;"),
        _ => return None,
    })
}

/// Emits a stack conversion from `from` to `to`. The value must already be on
/// top of the operand stack.
fn emit_convert<'a>(
    cp: &mut Cp<'a>,
    loader: &ClassLoader<'a>,
    code: &mut Vec<u8>,
    from: &Type,
    to: &Type,
) -> io::Result<()> {
    if from == to || to.is_void() {
        return Ok(());
    }
    match (from, to) {
        (Type::Ref(_), Type::Ref(to_desc)) => {
            if to_desc == "Ljava/lang/Object;" {
                return Ok(());
            }
            let target = to
                .internal_name()
                .ok_or_else(|| Error::other("Invalid checkcast target"))?;
            let idx = cp.class(loader, &target);
            code.push(0xc0); // checkcast
            emit_u16(code, idx);
        }
        (Type::Ref(_), Type::Prim(tag)) => {
            let (wrapper, method, mdesc) = wrapper_unbox(*tag)
                .ok_or_else(|| Error::other(format!("Cannot unbox to {}", *tag as char)))?;
            let cidx = cp.class(loader, wrapper);
            code.push(0xc0); // checkcast wrapper
            emit_u16(code, cidx);
            let midx = cp.method_ref(loader, wrapper, method, mdesc, false);
            code.push(0xb6); // invokevirtual
            emit_u16(code, midx);
        }
        (Type::Prim(from_tag), Type::Ref(_)) => {
            let (wrapper, method, pdesc, rdesc) = wrapper_box(*from_tag)
                .ok_or_else(|| Error::other(format!("Cannot box {}", *from_tag as char)))?;
            let midx =
                cp.method_ref(loader, wrapper, method, &format!("({pdesc}){rdesc}"), false);
            code.push(0xb8); // invokestatic
            emit_u16(code, midx);
        }
        (Type::Prim(a), Type::Prim(b)) => {
            if a != b {
                return Err(Error::other(format!(
                    "Unsupported primitive conversion {} -> {}",
                    *a as char, *b as char
                )));
            }
        }
    }
    Ok(())
}

/// Resolves the bootstrap method handle; returns true for
/// LambdaMetafactory.metafactory / altMetafactory.
fn is_lambda_bootstrap(klass: &InstanceKlass<'_>, bs_info: &BootstrapInfo) -> io::Result<bool> {
    let Constant::MethodHandle(_, ref_index) = klass.constant(bs_info.bsm_ref())? else {
        return Ok(false);
    };
    let [owner, name, _desc] = klass.memberRef(*ref_index, false)?;
    Ok(owner == b"java/lang/invoke/LambdaMetafactory"
        && (name == b"metafactory" || name == b"altMetafactory"))
}

fn method_type_arg(args: &[BootstrapArg], index: usize) -> Option<String> {
    match args.get(index) {
        Some(BootstrapArg::MethodType(desc)) => Some(desc.clone()),
        _ => None,
    }
}

fn impl_method_arg(klass: &InstanceKlass<'_>, args: &[BootstrapArg]) -> io::Result<ImplMethod> {
    let Some(BootstrapArg::MethodHandle { kind, index }) = args.get(1) else {
        return Err(Error::other("LambdaMetafactory: missing implMethod"));
    };
    let [owner, name, desc] = klass.memberRef(*index, false)?;
    Ok(ImplMethod {
        kind: *kind,
        owner: String::from_utf8_lossy(owner).into_owned(),
        name: String::from_utf8_lossy(name).into_owned(),
        descriptor: String::from_utf8_lossy(desc).into_owned(),
    })
}

/// Attempts to service an invokedynamic call site through LambdaMetafactory.
/// Returns `Ok(None)` when the bootstrap is not LambdaMetafactory.
pub fn invoke<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    indy_name: &[u8],
    indy_descriptor: &[u8],
    captured: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    if !is_lambda_bootstrap(klass, bs_info)? {
        return Ok(None);
    }

    let args = bs_info.arguments();
    let sam_desc = method_type_arg(args, 0)
        .ok_or_else(|| Error::other("LambdaMetafactory: missing samMethodType"))?;
    let impl_method = impl_method_arg(klass, args)?;

    // Call-site descriptor: (captures)LSamInterface;
    let callsite_desc = std::str::from_utf8(indy_descriptor)
        .map_err(|_| Error::other("Invalid invokedynamic descriptor"))?;
    let (capture_types, sam_ret) = Type::method(callsite_desc)
        .ok_or_else(|| Error::other("Invalid invokedynamic descriptor"))?;
    let Type::Ref(sam_interface) = &sam_ret else {
        return Err(Error::other(
            "LambdaMetafactory: call site does not return an interface",
        ));
    };
    let sam_interface = sam_interface
        .strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(sam_interface)
        .to_owned();

    let (sam_params, sam_return) = Type::method(&sam_desc)
        .ok_or_else(|| Error::other("Invalid samMethodType"))?;
    let (impl_params, impl_return) = Type::method(&impl_method.descriptor)
        .ok_or_else(|| Error::other("Invalid implMethod descriptor"))?;

    // Effective impl parameter list: for instance method refs the receiver is
    // the first parameter (a bound capture or the first SAM argument).
    let mut effective_params: Vec<Type> = Vec::new();
    match impl_method.kind {
        5 | 7 | 9 => effective_params.push(Type::Ref(format!("L{};", impl_method.owner))),
        6 | 8 => {}
        _ => return Err(Error::other("Unsupported implMethod reference kind")),
    }
    effective_params.extend(impl_params.iter().cloned());

    if capture_types.len() + sam_params.len() != effective_params.len() {
        return Err(Error::other(format!(
            "LambdaMetafactory arity mismatch: {} captures + {} sam params != {} impl params",
            capture_types.len(),
            sam_params.len(),
            effective_params.len()
        )));
    }
    if captured.len() != capture_types.len() {
        return Err(Error::other("LambdaMetafactory: capture count mismatch"));
    }

    // Cache lookup per (caller class, bootstrap-method index).
    let caller_name = klass.class.name;
    let bsm_index = bs_info.bss_index() as u16;
    let lambda_klass = if let Some(cached) = loader.lambda_class(caller_name, bsm_index) {
        cached
    } else {
        let generated = generate_class(
            loader,
            &sam_interface,
            &sam_desc,
            indy_name,
            &capture_types,
            &impl_method,
            &effective_params,
            &sam_params,
            &impl_return,
            &sam_return,
            args,
        )?;
        loader.cache_lambda_class(caller_name, bsm_index, Rc::clone(&generated));
        generated
    };

    // Instantiate: fields hold the captured call-site values.
    let object = crate::share::oops::instanceOop::InstanceOop::new(Rc::clone(&lambda_klass))?;
    let handle = loader.alloc_object(object);
    for (i, value) in captured.iter().enumerate() {
        let field_name = format!("arg{i}");
        let desc = capture_types[i].descriptor();
        let slot = {
            let objects = loader.borrow_objects();
            let obj = objects
                .get(handle)
                .ok_or_else(|| Error::other("Invalid lambda object"))?;
            obj.klass.instanceField(field_name.as_bytes(), desc.as_bytes())?
        };
        let mut objects = loader.borrow_objects_mut();
        let obj = objects
            .get_mut(handle)
            .ok_or_else(|| Error::other("Invalid lambda object"))?;
        if slot < obj.fields.len() {
            obj.fields[slot] = *value;
        }
    }
    Ok(Some(Value::Object(handle)))
}

#[allow(clippy::too_many_arguments)]
fn generate_class<'a>(
    loader: &ClassLoader<'a>,
    sam_interface: &str,
    sam_desc: &str,
    sam_name: &[u8],
    capture_types: &[Type],
    impl_method: &ImplMethod,
    effective_params: &[Type],
    sam_params: &[Type],
    impl_return: &Type,
    sam_return: &Type,
    bsm_args: &[BootstrapArg],
) -> io::Result<Rc<InstanceKlass<'a>>> {
    // altMetafactory flags: FLAG_MARKERS = 1, FLAG_SERIALIZABLE = 2,
    // FLAG_BRIDGES = 4. Marker interfaces come first, Serializable last —
    // matching LambdaMetafactory's generated interface list.
    let mut interfaces: Vec<&'a [u8]> = vec![loader.storage.alloc_str(sam_interface).as_bytes()];
    let flags = match bsm_args.get(3) {
        Some(BootstrapArg::Int(flags)) => *flags,
        _ => 0,
    };
    if flags & 1 != 0 {
        for arg in bsm_args.iter().skip(4) {
            if let BootstrapArg::Klass(name) = arg {
                interfaces.push(loader.storage.alloc_str(name).as_bytes());
            }
        }
    }
    if flags & 2 != 0 {
        interfaces.push(b"java/io/Serializable");
    }

    let counter = LAMBDA_COUNTER.fetch_add(1, Ordering::Relaxed);
    let class_name = format!("java/lang/invoke/Lambda${counter:016x}");
    let class_name_bytes: &'a [u8] = loader.storage.alloc_str(&class_name).as_bytes();

    let mut cp = Cp::new();

    // ---- Fields: one per captured call-site argument ----
    let mut fields = Vec::with_capacity(capture_types.len());
    for (i, ty) in capture_types.iter().enumerate() {
        let name = format!("arg{i}");
        fields.push(Member {
            accessFlags: 0x0012, // private final
            name: loader.storage.alloc_str(&name).as_bytes(),
            descriptor: loader.storage.alloc_str(&ty.descriptor()).as_bytes(),
            attributes: Vec::new(),
        });
    }

    // ---- <init>(captures)V ----
    let init_params: String = capture_types.iter().map(Type::descriptor).collect();
    let init_desc = format!("({init_params})V");
    let mut init_code = Vec::new();
    init_code.push(0x2a); // aload_0
    let object_init = cp.method_ref(loader, "java/lang/Object", "<init>", "()V", false);
    init_code.push(0xb7); // invokespecial
    emit_u16(&mut init_code, object_init);
    let mut local = 1usize;
    for (i, ty) in capture_types.iter().enumerate() {
        let field_name = format!("arg{i}");
        let fref = cp.field_ref(loader, &class_name, &field_name, &ty.descriptor());
        init_code.push(0x2a); // aload_0
        emit_load(&mut init_code, local, ty);
        local += ty.slots();
        init_code.push(0xb5); // putfield
        emit_u16(&mut init_code, fref);
    }
    init_code.push(0xb1); // return
    let init_max_locals = 1 + capture_types.iter().map(Type::slots).sum::<usize>();
    let init_member = Member {
        accessFlags: 0x0001, // public
        name: b"<init>",
        descriptor: loader.storage.alloc_str(&init_desc).as_bytes(),
        attributes: vec![code_attribute(loader, 4, init_max_locals, init_code)],
    };

    // ---- SAM method ----
    let sam_method_name: &'a [u8] =
        loader.storage.alloc_str(&String::from_utf8_lossy(sam_name)).as_bytes();
    let sam_method_desc: &'a [u8] = loader.storage.alloc_str(sam_desc).as_bytes();

    let mut code = Vec::new();
    if impl_method.kind == 8 {
        // new Owner; dup — leaves [uninit, uninit] for invokespecial <init>.
        let new_idx = cp.class(loader, &impl_method.owner);
        code.push(0xbb);
        emit_u16(&mut code, new_idx);
        code.push(0x59); // dup
    }
    // Captured args: aload_0; getfield arg_i; convert to effective_params[i].
    for (i, ty) in capture_types.iter().enumerate() {
        let field_name = format!("arg{i}");
        let fref = cp.field_ref(loader, &class_name, &field_name, &ty.descriptor());
        code.push(0x2a); // aload_0 (this, for getfield)
        code.push(0xb4); // getfield
        emit_u16(&mut code, fref);
        emit_convert(&mut cp, loader, &mut code, ty, &effective_params[i])?;
    }

    // SAM params.
    let mut local = 1usize;
    for (j, ty) in sam_params.iter().enumerate() {
        emit_load(&mut code, local, ty);
        local += ty.slots();
        emit_convert(
            &mut cp,
            loader,
            &mut code,
            ty,
            &effective_params[capture_types.len() + j],
        )?;
    }

    // Invoke impl.
    match impl_method.kind {
        5 | 6 | 7 => {
            let midx = cp.method_ref(
                loader,
                &impl_method.owner,
                &impl_method.name,
                &impl_method.descriptor,
                false,
            );
            let opcode = match impl_method.kind {
                5 => 0xb6,
                6 => 0xb8,
                _ => 0xb7,
            };
            code.push(opcode);
            emit_u16(&mut code, midx);
        }
        9 => {
            let midx = cp.method_ref(
                loader,
                &impl_method.owner,
                &impl_method.name,
                &impl_method.descriptor,
                true,
            );
            code.push(0xb9); // invokeinterface
            emit_u16(&mut code, midx);
            let (declared_params, _) = Type::method(&impl_method.descriptor)
                .ok_or_else(|| Error::other("Invalid impl descriptor"))?;
            let count = 1 + declared_params.iter().map(Type::slots).sum::<usize>();
            code.push(count as u8);
            code.push(0);
        }
        8 => {
            let midx = cp.method_ref(
                loader,
                &impl_method.owner,
                &impl_method.name,
                &impl_method.descriptor,
                false,
            );
            code.push(0xb7); // invokespecial <init>
            emit_u16(&mut code, midx);
        }
        _ => return Err(Error::other("Unsupported implMethod reference kind")),
    }

    // Return conversion impl_return -> sam_return.
    if impl_method.kind == 8 {
        // Constructor reference: <init> returns void; the constructed object
        // left on the stack by invokespecial is the SAM result.
        code.push(0xb0); // areturn
    } else if sam_return.is_void() {
        if !impl_return.is_void() {
            code.push(0x57); // pop
        }
        code.push(0xb1); // return
    } else {
        emit_convert(&mut cp, loader, &mut code, impl_return, &sam_return)?;
        code.push(match sam_return {
            Type::Prim(b'Z' | b'B' | b'C' | b'S' | b'I') => 0xac, // ireturn
            Type::Prim(b'J') => 0xad,                             // lreturn
            Type::Prim(b'F') => 0xae,                             // freturn
            Type::Prim(b'D') => 0xaf,                             // dreturn
            _ => 0xb0,                                            // areturn
        });
    }

    let sam_max_locals = 1 + sam_params.iter().map(Type::slots).sum::<usize>();
    let sam_max_stack = capture_types.len() + sam_params.len() + 4;
    let sam_member = Member {
        accessFlags: 0x0001, // public
        name: sam_method_name,
        descriptor: sam_method_desc,
        attributes: vec![code_attribute(loader, sam_max_stack, sam_max_locals, code)],
    };

    let class_file = ClassFile {
        majorVersion: 65,
        minorVersion: 0,
        accessFlags: 0x0031, // public final super
        name: class_name_bytes,
        superName: Some(b"java/lang/Object"),
        interfaces,
        constants: cp.entries,
        fields,
        methods: vec![init_member, sam_member],
        attributes: Vec::new(),
    };
    loader.define_class(class_file)
}
