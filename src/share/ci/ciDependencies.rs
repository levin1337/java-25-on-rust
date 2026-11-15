/*
 * Port of OpenJDK 25 HotSpot compiler dependencies
 * (share/ci/dependencies.hpp upstream; registered through
 * `ciEnv::register_*`).
 *
 * A compilation records assumptions ("this method stays final", "this
 * field stays constant", "this class hierarchy is stable").  On class
 * redefinition/linkage changes the dependencies are re-validated and
 * affected nmethods are deoptimized.  The port validates eagerly:
 * `DependencyRegistry::validate` re-checks every recorded assumption
 * against the current metadata.
 *
 * Method dependencies store a `CiMethodKey` (holder + name + signature)
 * rather than a `CiMethod` handle so they survive `CiMethod` handle
 * re-creation; `is_valid` re-resolves the method through `CiEnv`.
 */

use super::ciEnv::CiEnv;
use super::ciField::CiField;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciMethod::CiMethod;

/// Identity of a method for dependency purposes: holder klass plus
/// name+signature, re-resolvable through `CiEnv::lookup_method`.
#[derive(Debug, Clone)]
pub struct CiMethodKey<'a> {
    pub holder: CiInstanceKlass<'a>,
    pub name: Vec<u8>,
    pub signature: Vec<u8>,
}

impl<'a> CiMethodKey<'a> {
    pub fn new(method: &CiMethod<'a>) -> Self {
        Self {
            holder: method.holder().clone(),
            name: method.name().to_vec(),
            signature: method.signature().to_vec(),
        }
    }

    /// Re-resolve the method in the current metadata.
    pub fn resolve(&self, env: &CiEnv<'a>) -> Option<CiMethod<'a>> {
        env.lookup_method(&self.holder, &self.name, &self.signature)
    }
}

/// One recorded compilation dependency
/// (`Dependencies::Dependency` analogue).
#[derive(Clone)]
pub enum CiDependency<'a> {
    /// `ciMethod::can_be_statically_bound` must keep holding: the
    /// method is final/private/static and cannot be overridden.
    MethodStable { method: CiMethodKey<'a> },
    /// The method is the unique implementation of its signature in
    /// the hierarchy (`Dependencies::unique_method_dependency`).
    UniqueMethod { method: CiMethodKey<'a> },
    /// The field is a compile-time constant (`ConstantValue`
    /// attribute or stable static final).
    FieldConst { field: CiField<'a> },
    /// No subclass of `klass` may appear (`leaf type` assumption).
    LeafKlass { klass: CiInstanceKlass<'a> },
    /// `klass` keeps its current direct superclass.
    SuperKlass {
        klass: CiInstanceKlass<'a>,
        super_name: Vec<u8>,
    },
    /// The klass remains initialized (`ciEnv` initialization
    /// dependency analogue).
    ClassInitialized { klass: CiInstanceKlass<'a> },
}

impl<'a> CiDependency<'a> {
    /// Re-validate the assumption against current metadata.
    /// Returns `false` when the dependency is violated (the compiled
    /// code must be deoptimized).
    pub fn is_valid(&self, env: &CiEnv<'a>) -> bool {
        match self {
            Self::MethodStable { method } => method
                .resolve(env)
                .is_some_and(|m| m.can_be_statically_bound()),
            Self::UniqueMethod { method } => {
                // Unique: exactly one declaration site for this
                // name+signature across the holder's superclass chain
                // and transitive interfaces, and it is the canonical
                // holder of the resolved method.
                let Some(resolved) = method.resolve(env) else {
                    return false;
                };
                let canonical = resolved.holder().name();
                let mut declarations = 0usize;
                let mut current = Some(method.holder.clone());
                while let Some(k) = current {
                    if k.find_method(&method.name, &method.signature).is_some() {
                        declarations += 1;
                        if k.name() != canonical {
                            return false;
                        }
                    }
                    current = k.super_klass();
                }
                for iface in method.holder.transitive_interfaces() {
                    if iface
                        .find_method(&method.name, &method.signature)
                        .is_some()
                    {
                        declarations += 1;
                        if iface.name() != canonical {
                            return false;
                        }
                    }
                }
                declarations == 1
            }
            Self::FieldConst { field } => {
                field.constant_value().is_some() || field.is_final()
            }
            Self::LeafKlass { klass } => !klass.has_subklass() || klass.is_final(),
            Self::SuperKlass { klass, super_name } => klass
                .super_klass()
                .is_some_and(|s| s.name() == super_name.as_slice()),
            Self::ClassInitialized { klass } => klass.is_initialized(),
        }
    }

    /// Short description for `-XX:+PrintDependencies` output.
    pub fn describe(&self) -> String {
        match self {
            Self::MethodStable { method } | Self::UniqueMethod { method } => format!(
                "method {}.{}{}",
                String::from_utf8_lossy(method.holder.name()),
                String::from_utf8_lossy(&method.name),
                String::from_utf8_lossy(&method.signature)
            ),
            Self::FieldConst { field } => format!(
                "field {}.{}{}",
                String::from_utf8_lossy(field.holder().name()),
                String::from_utf8_lossy(field.name()),
                String::from_utf8_lossy(field.signature())
            ),
            Self::LeafKlass { klass } => format!("{} is a leaf class", klass.name_string()),
            Self::SuperKlass { klass, super_name } => format!(
                "{} superclass is {}",
                klass.name_string(),
                String::from_utf8_lossy(super_name)
            ),
            Self::ClassInitialized { klass } => {
                format!("{} is initialized", klass.name_string())
            }
        }
    }
}

/// `ciEnv::dependencies` analogue: the set of assumptions recorded for
/// one compilation (or the whole env when shared).
#[derive(Default)]
pub struct DependencyRegistry<'a> {
    deps: Vec<CiDependency<'a>>,
}

impl<'a> DependencyRegistry<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a dependency.
    pub fn register(&mut self, dep: CiDependency<'a>) {
        self.deps.push(dep);
    }

    /// `Dependencies::Method::register`-style helper: record that
    /// `method` may be treated as a compile-time constant target.
    pub fn register_method(&mut self, method: &CiMethod<'a>) {
        let key = CiMethodKey::new(method);
        if method.can_be_statically_bound() {
            self.register(CiDependency::MethodStable { method: key.clone() });
        }
        self.register(CiDependency::UniqueMethod { method: key });
    }

    /// `Dependencies::Field::register` analogue.
    pub fn register_field(&mut self, field: CiField<'a>) {
        if field.constant_value().is_some() {
            self.register(CiDependency::FieldConst { field });
        }
    }

    /// `Dependencies::LeafType` analogue.
    pub fn register_leaf_klass(&mut self, klass: CiInstanceKlass<'a>) {
        self.register(CiDependency::LeafKlass { klass });
    }

    /// `ciEnv::register_initialize_dependence` analogue.
    pub fn register_initialized(&mut self, klass: CiInstanceKlass<'a>) {
        self.register(CiDependency::ClassInitialized { klass });
    }

    /// All recorded dependencies.
    pub fn dependencies(&self) -> &[CiDependency<'a>] {
        &self.deps
    }

    /// `Dependencies::validate_dependencies`: returns the indices of
    /// violated dependencies (empty = all still valid).
    pub fn validate(&self, env: &CiEnv<'a>) -> Vec<usize> {
        self.deps
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.is_valid(env))
            .map(|(i, _)| i)
            .collect()
    }

    /// Whether every recorded dependency still holds.
    pub fn is_valid(&self, env: &CiEnv<'a>) -> bool {
        self.validate(env).is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }

    pub fn clear(&mut self) {
        self.deps.clear();
    }
}
