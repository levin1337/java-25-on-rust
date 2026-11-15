/*
 * Port of OpenJDK 25 HotSpot Dependencies (share/code/dependencies.hpp /
 * dependencies.cpp).
 *
 * An nmethod records "dependencies" — assumptions about the class
 * hierarchy and call-site targets that were true when the code was
 * compiled.  When a class is loaded, redefined or unloaded, the runtime
 * walks every nmethod's DependencySet and marks dependent code for
 * deoptimization (`Dependencies::check_dependency` /
 * `DependencyContext`).
 *
 * HotSpot serializes dependencies into the nmethod's immutable data
 * section as (DepType, argument indexes) tuples.  Here dependencies are
 * kept as typed records with structural metadata arguments
 * (klass/method/field/call-site), which is sufficient for the
 * invalidation logic the code cache performs.
 */

/// `Dependencies::DepType` (dependencies.hpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum DepType {
    /// Sentinel; must not appear in a real dependency.
    Undefined = -1,
    /// Marks the end of a serialized dependency list.
    EndMarker = 0,
    /// The contents of the method were used; if it evolves the nmethod
    /// must be recompiled.  No other dependencies implied.
    EvolMethod = 1,
    /// Context type CX is a leaf type (no proper subtype).
    LeafType = 2,
    /// Abstract class CX has exactly one concrete subtype CC.
    AbstractWithUniqueConcreteSubtype = 3,
    /// MM(CX, M1) has at most one concrete method under CX
    /// (method + context class arguments).
    UniqueConcreteMethod2 = 4,
    /// Like UniqueConcreteMethod2 but with resolved class/method
    /// arguments for precision.
    UniqueConcreteMethod4 = 5,
    /// Interface CX has a unique implementor class.
    UniqueImplementor = 6,
    /// No instances of class or its subclasses require finalization.
    NoFinalizableSubclasses = 7,
    /// The CallSite.target value changed.
    CallSiteTargetValue = 8,
}

impl DepType {
    /// `Dependencies::is_klass_type`: dependencies whose first argument
    /// is a context klass type.
    pub fn is_klass_type(self) -> bool {
        matches!(
            self,
            Self::LeafType
                | Self::AbstractWithUniqueConcreteSubtype
                | Self::UniqueConcreteMethod2
                | Self::UniqueConcreteMethod4
                | Self::UniqueImplementor
                | Self::NoFinalizableSubclasses
        )
    }

    /// Number of arguments the dependency type takes
    /// (`Dependencies::dep_args` analogue, structural view).
    pub fn argument_count(self) -> usize {
        match self {
            Self::Undefined | Self::EndMarker => 0,
            Self::EvolMethod => 1, // method
            Self::LeafType | Self::UniqueImplementor | Self::NoFinalizableSubclasses => 1, // klass
            Self::AbstractWithUniqueConcreteSubtype => 2, // klass CX + klass CC
            Self::UniqueConcreteMethod2 => 2, // method M1 + klass CX
            Self::UniqueConcreteMethod4 => 4, // M1, CX, RC1, RM1
            Self::CallSiteTargetValue => 1, // call site target oop
        }
    }
}

/// A metadata argument of a dependency.  HotSpot records raw Metadata*
/// pointers / oops; here they are structural identities so the port
/// works without a live heap.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DependencyArgument {
    /// Klass context type (InstanceKlass identity).
    Klass(String),
    /// Method (holder.name+signature identity).
    Method {
        holder: String,
        name: String,
        sig: String,
    },
    /// Field (holder, name, signature, offset).
    Field {
        holder: String,
        name: String,
        sig: String,
        offset: i32,
    },
    /// Resolved call-site target oop (e.g. a CallSite/MethodHandle).
    CallSiteTarget(String),
}

impl DependencyArgument {
    /// The klass argument for context-type dependencies.
    pub fn as_klass(&self) -> Option<&str> {
        match self {
            Self::Klass(k) => Some(k),
            _ => None,
        }
    }

    /// The method argument.
    pub fn as_method(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::Method { holder, name, sig } => Some((holder, name, sig)),
            _ => None,
        }
    }

    /// `Dependencies::is_oop_argument`: only call_site_target_value
    /// arguments are oops.
    pub fn is_oop(&self) -> bool {
        matches!(self, Self::CallSiteTarget(_))
    }
}

/// One recorded dependency (`Dependencies::Dependency` analogue).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    dep_type: DepType,
    args: Vec<DependencyArgument>,
}

impl Dependency {
    pub fn new(dep_type: DepType, args: Vec<DependencyArgument>) -> Self {
        debug_assert_eq!(dep_type.argument_count(), args.len());
        Self { dep_type, args }
    }

    /// Constructor without arity assertion for decoded/foreign data.
    pub fn new_unchecked(dep_type: DepType, args: Vec<DependencyArgument>) -> Self {
        Self { dep_type, args }
    }

    #[inline]
    pub fn type_(&self) -> DepType {
        self.dep_type
    }

    #[inline]
    pub fn args(&self) -> &[DependencyArgument] {
        &self.args
    }

    /// The context type (first klass argument) for klass-type deps.
    pub fn context_type(&self) -> Option<&str> {
        if !self.dep_type.is_klass_type() {
            return None;
        }
        self.args.first().and_then(|a| a.as_klass())
    }

    /// `Dependency::is_oop_argument` analogue.
    pub fn is_oop_argument(&self, i: usize) -> bool {
        self.dep_type == DepType::CallSiteTargetValue && i == 0
    }

    /// Whether this dependency is affected by a change to `klass_name`
    /// (`Dependencies::check_klass_dependency` analogue): the changed
    /// class must be the context type or a subtype of it.  Subtype
    /// information is supplied by the caller through `is_subtype`.
    pub fn affected_by_klass<F>(&self, klass_name: &str, is_subtype: F) -> bool
    where
        F: Fn(&str, &str) -> bool,
    {
        match self.dep_type {
            DepType::CallSiteTargetValue => false,
            DepType::EvolMethod => {
                // Evol deps track a method; a klass change affects them
                // only if the changed klass is the method's holder or a
                // subtype of it.
                self.args.iter().any(|a| match a {
                    DependencyArgument::Method { holder, .. } => {
                        holder == klass_name || is_subtype(klass_name, holder)
                    }
                    _ => false,
                })
            }
            _ => self.args.iter().any(|a| match a {
                // The changed klass invalidates the dependency when it
                // IS the context type or a SUBTYPE of it
                // (Dependencies::check_klass_dependency).
                DependencyArgument::Klass(k) => k == klass_name || is_subtype(klass_name, k),
                DependencyArgument::Method { holder, .. } => {
                    holder == klass_name || is_subtype(klass_name, holder)
                }
                DependencyArgument::Field { holder, .. } => {
                    holder == klass_name || is_subtype(klass_name, holder)
                }
                DependencyArgument::CallSiteTarget(_) => false,
            }),
        }
    }
}

/// A set of dependencies recorded by one nmethod
/// (`Dependencies` content analogue).
#[derive(Clone, Debug, Default)]
pub struct DependencySet {
    deps: Vec<Dependency>,
}

impl DependencySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            deps: Vec::with_capacity(cap),
        }
    }

    /// `Dependencies::add_dependency` analogue: appends if not already
    /// present (HotSpot dedups on (type,args) via DependencySignature).
    pub fn add(&mut self, dep: Dependency) {
        if !self.deps.contains(&dep) {
            self.deps.push(dep);
        }
    }

    pub fn add_evol_method(&mut self, holder: &str, name: &str, sig: &str) {
        self.add(Dependency::new(
            DepType::EvolMethod,
            vec![DependencyArgument::Method {
                holder: holder.to_string(),
                name: name.to_string(),
                sig: sig.to_string(),
            }],
        ));
    }

    pub fn add_leaf_type(&mut self, klass: &str) {
        self.add(Dependency::new(
            DepType::LeafType,
            vec![DependencyArgument::Klass(klass.to_string())],
        ));
    }

    pub fn add_unique_implementor(&mut self, interface: &str) {
        self.add(Dependency::new(
            DepType::UniqueImplementor,
            vec![DependencyArgument::Klass(interface.to_string())],
        ));
    }

    pub fn add_unique_concrete_method(
        &mut self,
        holder: &str,
        name: &str,
        sig: &str,
        context: &str,
    ) {
        self.add(Dependency::new(
            DepType::UniqueConcreteMethod2,
            vec![
                DependencyArgument::Method {
                    holder: holder.to_string(),
                    name: name.to_string(),
                    sig: sig.to_string(),
                },
                DependencyArgument::Klass(context.to_string()),
            ],
        ));
    }

    pub fn add_call_site_target(&mut self, target: &str) {
        self.add(Dependency::new(
            DepType::CallSiteTargetValue,
            vec![DependencyArgument::CallSiteTarget(target.to_string())],
        ));
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.deps.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Dependency> {
        self.deps.iter()
    }

    pub fn dependencies(&self) -> &[Dependency] {
        &self.deps
    }

    /// Remove all dependencies (`nmethod::flush_dependencies` analogue).
    pub fn clear(&mut self) {
        self.deps.clear();
    }

    /// Whether any dependency references `klass_name` as context type or
    /// argument holder.  `is_subtype(a, b)` reports whether klass `a` is
    /// a subtype of `b`.
    pub fn has_dependency_on<F>(&self, klass_name: &str, is_subtype: F) -> bool
    where
        F: Fn(&str, &str) -> bool,
    {
        self.deps
            .iter()
            .any(|d| d.affected_by_klass(klass_name, &is_subtype))
    }

    /// `Dependencies::check_dependency` analogue over the whole set:
    /// returns the first dependency invalidated by a change to
    /// `klass_name`, or None if all remain valid.
    pub fn first_invalidated_by<F>(&self, klass_name: &str, is_subtype: F) -> Option<&Dependency>
    where
        F: Fn(&str, &str) -> bool,
    {
        self.deps
            .iter()
            .find(|d| d.affected_by_klass(klass_name, &is_subtype))
    }

    /// Estimated serialized size in bytes (type + arg count + args).
    pub fn size_in_bytes(&self) -> usize {
        self.deps.iter().map(|d| 8 + d.args.len() * 16).sum()
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        if self.deps.is_empty() {
            let _ = writeln!(f, "  no dependencies");
            return;
        }
        let _ = writeln!(f, "  dependencies ({}):", self.deps.len());
        for d in &self.deps {
            let _ = write!(f, "    {:?}", d.type_());
            for a in d.args() {
                let _ = write!(f, " ");
                match a {
                    DependencyArgument::Klass(k) => {
                        let _ = write!(f, "klass:{}", k);
                    }
                    DependencyArgument::Method { holder, name, sig } => {
                        let _ = write!(f, "method:{}.{ }{}", holder, name, sig);
                    }
                    DependencyArgument::Field {
                        holder,
                        name,
                        sig,
                        offset,
                    } => {
                        let _ = write!(f, "field:{}#{}{}@{}", holder, name, sig, offset);
                    }
                    DependencyArgument::CallSiteTarget(t) => {
                        let _ = write!(f, "call_site_target:{}", t);
                    }
                }
            }
            let _ = writeln!(f);
        }
    }
}

/// Context in which dependencies are validated
/// (`Dependencies::DependencyContext` analogue): records which klasses
/// changed and offers subtype queries to `check_dependency`.
#[derive(Clone, Debug, Default)]
pub struct DependencyContext {
    /// Klasses that were loaded/linked/redefined since the last check.
    changed_klasses: Vec<String>,
    /// (sub, super) pairs known to hold for subtype queries.
    subtype_edges: Vec<(String, String)>,
}

impl DependencyContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            changed_klasses: Vec::with_capacity(cap),
            subtype_edges: Vec::with_capacity(cap),
        }
    }

    pub fn add_changed_klass(&mut self, klass: &str) {
        if !self.changed_klasses.iter().any(|k| k == klass) {
            self.changed_klasses.push(klass.to_string());
        }
    }

    pub fn add_subtype_edge(&mut self, sub: &str, sup: &str) {
        self.subtype_edges.push((sub.to_string(), sup.to_string()));
    }

    /// Transitive subtype query over recorded edges.
    pub fn is_subtype(&self, sub: &str, sup: &str) -> bool {
        if sub == sup {
            return true;
        }
        // BFS over subtype edges.
        let mut frontier = vec![sub.to_string()];
        let mut seen = std::collections::HashSet::new();
        seen.insert(sub.to_string());
        while let Some(cur) = frontier.pop() {
            for (s, p) in &self.subtype_edges {
                if s == &cur && !seen.contains(p) {
                    if p == sup {
                        return true;
                    }
                    seen.insert(p.clone());
                    frontier.push(p.clone());
                }
            }
        }
        false
    }

    pub fn changed_klasses(&self) -> &[String] {
        &self.changed_klasses
    }

    /// Find all dependencies in `set` invalidated by the recorded
    /// changes, returning (dependency, witness klass) pairs.
    pub fn invalidated<'a>(&self, set: &'a DependencySet) -> Vec<(&'a Dependency, String)> {
        let mut out = Vec::new();
        for klass in &self.changed_klasses {
            if let Some(dep) = set.first_invalidated_by(klass, |a, b| self.is_subtype(a, b)) {
                out.push((dep, klass.clone()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_type_arity() {
        assert_eq!(DepType::EvolMethod.argument_count(), 1);
        assert_eq!(DepType::UniqueConcreteMethod4.argument_count(), 4);
        assert_eq!(DepType::CallSiteTargetValue.argument_count(), 1);
        assert!(DepType::LeafType.is_klass_type());
        assert!(!DepType::EvolMethod.is_klass_type());
        assert!(!DepType::CallSiteTargetValue.is_klass_type());
    }

    #[test]
    fn dependency_set_dedup_and_invalidation() {
        let mut set = DependencySet::new();
        set.add_leaf_type("Base");
        set.add_leaf_type("Base"); // duplicate: dropped
        set.add_unique_implementor("Iface");
        assert_eq!(set.len(), 2);

        let mut ctx = DependencyContext::new();
        ctx.add_subtype_edge("Sub", "Base");
        ctx.add_changed_klass("Sub");
        // 'Sub' is a subtype of 'Base' -> the leaf_type dep on Base is
        // invalidated (a new leaf appeared under the context type).
        let bad = ctx.invalidated(&set);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0.type_(), DepType::LeafType);
        assert_eq!(bad[0].1, "Sub");

        // Unrelated change: nothing invalidated.
        let mut ctx2 = DependencyContext::new();
        ctx2.add_changed_klass("Unrelated");
        assert!(ctx2.invalidated(&set).is_empty());
    }

    #[test]
    fn evol_method_dependency() {
        let mut set = DependencySet::new();
        set.add_evol_method("pkg/K", "m", "()V");
        let mut ctx = DependencyContext::new();
        ctx.add_changed_klass("pkg/K");
        assert_eq!(ctx.invalidated(&set).len(), 1);
    }
}
