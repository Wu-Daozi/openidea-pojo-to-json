//! Java source parsing: locate a class/record, collect its fields, and build
//! a JSON skeleton with defaults derived from the declared types.
//!
//! Types are first resolved inside the same compilation unit. When that fails,
//! an optional resolver callback can provide the source of another Java file
//! (the plugin uses the project index for that), so a `List<Pojo>` field can
//! expand to a one-element array with the nested object's skeleton.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use java_lang::Span;
use java_lang::ast::{
    Annotation, ClassBodyDecl, ClassDecl, CompilationUnit, ElementValue, EnumDecl, Expr, FieldDecl,
    ImportDecl, Lit, Modifier, RecordBodyDecl, RecordComponent, RecordDecl, ReferenceType, Type,
    TypeArgument, TypeDecl, VariableDeclarator, WildcardBound,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::defaults::{self, Now};
use crate::options::{NamingStrategy, Options, UnknownType};

#[derive(Debug)]
pub(crate) struct Generated {
    pub(crate) class_name: String,
    pub(crate) field_count: usize,
    pub(crate) json: String,
}

#[derive(Debug)]
pub(crate) enum GenerateError {
    /// The file has several top-level classes and there was no cursor to pick one.
    NeedChoice(Vec<String>),
    Failed(String),
}

/// Package and single-type imports of the target file, used to disambiguate
/// external types with the same simple name.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolutionHints {
    pub(crate) package: Option<String>,
    pub(crate) imports: HashMap<String, String>,
    pub(crate) wildcard_packages: Vec<String>,
}

impl ResolutionHints {
    pub(crate) fn qualified(&self, name: &str) -> Option<String> {
        self.imports.get(name).cloned().or_else(|| {
            self.package
                .as_ref()
                .map(|package| format!("{package}.{name}"))
        })
    }

    pub(crate) fn key(&self, name: &str) -> String {
        self.qualified(name).unwrap_or_else(|| name.to_owned())
    }
}

pub(crate) fn resolution_hints(source: &str) -> ResolutionHints {
    let Ok(unit) = parse(source) else {
        return ResolutionHints::default();
    };
    let package = unit.package().map(|package| path_string(&package.name));
    let mut imports = HashMap::new();
    let mut wildcard_packages = Vec::new();

    for import in &unit.imports {
        match import {
            ImportDecl::SingleType { path, .. } => {
                imports.insert(path.last_ident().name.clone(), path_string(path));
            }
            ImportDecl::TypeOnDemand { path, .. } => {
                wildcard_packages.push(path_string(path));
            }
            _ => {}
        }
    }

    ResolutionHints {
        package,
        imports,
        wildcard_packages,
    }
}

/// Whether `source` declares a class/record/enum named `name` in the expected
/// package. Used to validate candidate files found by path globs.
pub(crate) fn declares_type(source: &str, name: &str, expected_package: Option<&str>) -> bool {
    let Ok(unit) = parse(source) else {
        return false;
    };
    let package = unit.package().map(|package| path_string(&package.name));
    if let Some(expected) = expected_package
        && package.as_deref() != Some(expected)
    {
        return false;
    }
    models_from_unit(&unit, &Options::default(), &ResolutionHints::default())
        .iter()
        .any(|model| model.name == name)
}

fn path_string(path: &java_lang::ast::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
pub(crate) fn generate(source: &str, offset: Option<usize>) -> Result<Generated, GenerateError> {
    generate_with(
        source,
        offset,
        &mut |_, _| None,
        &Options::default(),
        &Now::epoch(),
    )
}

pub(crate) fn generate_with(
    source: &str,
    offset: Option<usize>,
    resolve: &mut dyn FnMut(&str, &ResolutionHints) -> Option<String>,
    options: &Options,
    now: &Now,
) -> Result<Generated, GenerateError> {
    let unit = parse(source)?;
    let hints = resolution_hints(source);
    let all = all_targets(&unit);
    let top = top_targets(&unit);
    let target = locate_target(&all, &top, offset)?;
    let model = target_model(target, options, &hints);
    let mut index = ModelIndex::from_unit(&unit, options, &hints);
    build(&model, &mut index, resolve, options, now)
}

#[cfg(test)]
pub(crate) fn generate_named(source: &str, name: &str) -> Result<Generated, GenerateError> {
    generate_named_with(
        source,
        name,
        &mut |_, _| None,
        &Options::default(),
        &Now::epoch(),
    )
}

pub(crate) fn generate_named_with(
    source: &str,
    name: &str,
    resolve: &mut dyn FnMut(&str, &ResolutionHints) -> Option<String>,
    options: &Options,
    now: &Now,
) -> Result<Generated, GenerateError> {
    let unit = parse(source)?;
    let hints = resolution_hints(source);
    let all = all_targets(&unit);
    let target = all
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| GenerateError::Failed(format!("Class {name} not found in this file")))?;
    let model = target_model(target, options, &hints);
    let mut index = ModelIndex::from_unit(&unit, options, &hints);
    build(&model, &mut index, resolve, options, now)
}

fn parse(source: &str) -> Result<CompilationUnit, GenerateError> {
    java_lang::parse_str::<CompilationUnit>(source).map_err(|e| {
        let pos = openidea_plugin::text::position(source, e.span().start());
        GenerateError::Failed(format!(
            "Java parse error at line {}, column {}: {}",
            pos.line + 1,
            pos.character + 1,
            e.message()
        ))
    })
}

#[derive(Clone, Copy)]
enum TargetBody<'a> {
    Class(&'a ClassDecl),
    Record(&'a RecordDecl),
}

struct Target<'a> {
    name: String,
    span: Span,
    body: TargetBody<'a>,
}

fn class_target(c: &ClassDecl) -> Target<'_> {
    Target {
        name: c.name.name.clone(),
        span: c.span(),
        body: TargetBody::Class(c),
    }
}

fn record_target(r: &RecordDecl) -> Target<'_> {
    Target {
        name: r.name.name.clone(),
        span: r.span(),
        body: TargetBody::Record(r),
    }
}

fn top_targets(unit: &CompilationUnit) -> Vec<Target<'_>> {
    let mut out = Vec::new();
    for decl in unit.type_decls() {
        match decl {
            TypeDecl::Class(c) => out.push(class_target(c)),
            TypeDecl::Record(r) => out.push(record_target(r)),
            _ => {}
        }
    }
    out
}

fn all_targets(unit: &CompilationUnit) -> Vec<Target<'_>> {
    let mut out = Vec::new();
    collect_from_decls(unit.type_decls(), &mut out);
    out
}

fn collect_from_decls<'a>(decls: &'a [TypeDecl], out: &mut Vec<Target<'a>>) {
    for decl in decls {
        match decl {
            TypeDecl::Class(c) => {
                out.push(class_target(c));
                collect_from_class_body(&c.body.declarations, out);
            }
            TypeDecl::Record(r) => {
                out.push(record_target(r));
                collect_from_record_body(&r.body.members, out);
            }
            TypeDecl::Enum(e) => collect_from_class_body(&e.body.members, out),
            _ => {}
        }
    }
}

fn collect_from_class_body<'a>(decls: &'a [ClassBodyDecl], out: &mut Vec<Target<'a>>) {
    for decl in decls {
        match decl {
            ClassBodyDecl::Class(c) => {
                out.push(class_target(c));
                collect_from_class_body(&c.body.declarations, out);
            }
            ClassBodyDecl::Record(r) => {
                out.push(record_target(r));
                collect_from_record_body(&r.body.members, out);
            }
            ClassBodyDecl::Enum(e) => collect_from_class_body(&e.body.members, out),
            _ => {}
        }
    }
}

fn collect_from_record_body<'a>(decls: &'a [RecordBodyDecl], out: &mut Vec<Target<'a>>) {
    for decl in decls {
        match decl {
            RecordBodyDecl::Class(c) => {
                out.push(class_target(c));
                collect_from_class_body(&c.body.declarations, out);
            }
            RecordBodyDecl::Record(r) => {
                out.push(record_target(r));
                collect_from_record_body(&r.body.members, out);
            }
            RecordBodyDecl::Enum(e) => collect_from_class_body(&e.body.members, out),
            _ => {}
        }
    }
}

fn locate_target<'s, 'a>(
    all: &'s [Target<'a>],
    top: &'s [Target<'a>],
    offset: Option<usize>,
) -> Result<&'s Target<'a>, GenerateError> {
    let containing = offset.and_then(|at| {
        all.iter()
            .filter(|target| target.span.start() <= at && at < target.span.end())
            .min_by_key(|target| target.span.end().saturating_sub(target.span.start()))
    });
    match containing {
        Some(target) => Ok(target),
        None => match top {
            [] => Err(GenerateError::Failed(
                "No Java class or record found in this file".into(),
            )),
            [only] => Ok(only),
            many => Err(GenerateError::NeedChoice(
                many.iter().map(|target| target.name.clone()).collect(),
            )),
        },
    }
}

fn target_model(target: &Target<'_>, options: &Options, hints: &ResolutionHints) -> Model {
    match target.body {
        TargetBody::Class(class) => model_from_class(class, options, hints),
        TargetBody::Record(record) => model_from_record(record, options, hints),
    }
}

#[derive(Clone, Debug)]
enum TypeRef {
    Primitive(String),
    Array(Box<TypeRef>),
    Named { name: String, args: Vec<TypeRef> },
    Unknown,
}

#[derive(Clone, Debug)]
struct Field {
    java_name: String,
    explicit_json_name: Option<String>,
    ty: TypeRef,
}

#[derive(Clone, Debug)]
struct Model {
    name: String,
    qualified_name: String,
    hints: ResolutionHints,
    fields: Vec<Field>,
    enum_constants: Vec<String>,
    is_enum: bool,
    extends: Option<TypeRef>,
    ignore_type: bool,
    naming: NamingStrategy,
    ignored_properties: HashSet<String>,
}

struct CollectedField {
    name: String,
    java_name: String,
    ty: TypeRef,
    hints: ResolutionHints,
}

fn model_from_class(class: &ClassDecl, options: &Options, hints: &ResolutionHints) -> Model {
    let naming = class_naming(&class.modifiers, options);
    let mut fields = Vec::new();
    for declaration in &class.body.declarations {
        if let ClassBodyDecl::Field(field) = declaration {
            fields.extend(field_models(field, options));
        }
    }

    Model {
        name: class.name.name.clone(),
        qualified_name: hints.key(&class.name.name),
        hints: hints.clone(),
        fields,
        enum_constants: Vec::new(),
        is_enum: false,
        extends: class
            .extends_clause
            .as_ref()
            .map(|clause| type_ref(&clause.supertype)),
        ignore_type: has_annotation(&class.modifiers, "JsonIgnoreType"),
        naming,
        ignored_properties: class_ignored_properties(&class.modifiers),
    }
}

fn model_from_record(record: &RecordDecl, options: &Options, hints: &ResolutionHints) -> Model {
    let naming = class_naming(&record.modifiers, options);
    let fields = record
        .components
        .components
        .iter()
        .filter_map(|component| record_field(component, options))
        .collect();

    Model {
        name: record.name.name.clone(),
        qualified_name: hints.key(&record.name.name),
        hints: hints.clone(),
        fields,
        enum_constants: Vec::new(),
        is_enum: false,
        extends: None,
        ignore_type: has_annotation(&record.modifiers, "JsonIgnoreType"),
        naming,
        ignored_properties: class_ignored_properties(&record.modifiers),
    }
}

fn model_from_enum(enum_decl: &EnumDecl, options: &Options, hints: &ResolutionHints) -> Model {
    Model {
        name: enum_decl.name.name.clone(),
        qualified_name: hints.key(&enum_decl.name.name),
        hints: hints.clone(),
        fields: Vec::new(),
        enum_constants: enum_decl
            .body
            .constants
            .iter()
            .map(|constant| constant.name.name.clone())
            .collect(),
        is_enum: true,
        extends: None,
        ignore_type: has_annotation(&enum_decl.modifiers, "JsonIgnoreType"),
        naming: options.naming,
        ignored_properties: class_ignored_properties(&enum_decl.modifiers),
    }
}

fn field_models(field: &FieldDecl, options: &Options) -> Vec<Field> {
    if has_static(&field.modifiers) && !options.include_static {
        return Vec::new();
    }
    if has_transient(&field.modifiers) && !options.include_transient {
        return Vec::new();
    }

    let annotations = annotations_from_modifiers(&field.modifiers);
    if is_json_ignored(&annotations) || json_access_is_write_only(&annotations) {
        return Vec::new();
    }
    let explicit_json_name = json_property_name(&annotations);
    let base = type_ref(&field.ty);

    field
        .declarators
        .iter()
        .filter(|declarator| {
            !(has_static(&field.modifiers)
                && declarator
                    .name
                    .as_ref()
                    .is_some_and(|name| name.name == "serialVersionUID"))
        })
        .filter_map(|declarator| field_from_declarator(declarator, &base, &explicit_json_name))
        .collect()
}

fn field_from_declarator(
    declarator: &VariableDeclarator,
    base: &TypeRef,
    explicit_json_name: &Option<String>,
) -> Option<Field> {
    let name = declarator.name.as_ref()?;
    let mut ty = base.clone();
    for _ in &declarator.dims {
        ty = TypeRef::Array(Box::new(ty));
    }
    Some(Field {
        java_name: name.name.clone(),
        explicit_json_name: explicit_json_name.clone(),
        ty,
    })
}

fn record_field(component: &RecordComponent, _options: &Options) -> Option<Field> {
    let (annotations, ty, name, varargs) = match component {
        RecordComponent::Normal {
            annotations,
            ty,
            name,
        } => (annotations, ty, name, false),
        RecordComponent::VarArgs {
            annotations,
            ty,
            name,
            ..
        } => (annotations, ty, name, true),
    };

    let refs: Vec<&Annotation> = annotations.iter().collect();
    if is_json_ignored(&refs) || json_access_is_write_only(&refs) {
        return None;
    }
    let ty = type_ref(ty);
    Some(Field {
        java_name: name.name.clone(),
        explicit_json_name: json_property_name(&refs),
        ty: if varargs {
            TypeRef::Array(Box::new(ty))
        } else {
            ty
        },
    })
}

fn type_ref(ty: &Type) -> TypeRef {
    match ty {
        Type::Primitive(primitive) => TypeRef::Primitive(primitive.kind.as_str().to_owned()),
        Type::Void(_) => TypeRef::Unknown,
        Type::Reference(reference) => match reference {
            ReferenceType::Array(array) => TypeRef::Array(Box::new(type_ref(&array.elem_type))),
            ReferenceType::TypeVar(_) => TypeRef::Unknown,
            ReferenceType::ClassOrInterfaceType(class) => {
                let args = class
                    .type_args()
                    .map(|args| args.args.iter().map(type_arg_ref).collect())
                    .unwrap_or_default();
                TypeRef::Named {
                    name: class.path.last_ident().name.clone(),
                    args,
                }
            }
        },
    }
}

fn type_arg_ref(arg: &TypeArgument) -> TypeRef {
    match arg {
        TypeArgument::Type(ty) => type_ref(ty.as_ref()),
        TypeArgument::Wildcard(wildcard) => match &wildcard.bound {
            Some(WildcardBound::Extends(ty)) | Some(WildcardBound::Super(ty)) => {
                type_ref(ty.as_ref())
            }
            None => TypeRef::Unknown,
        },
    }
}

fn models_from_unit(
    unit: &CompilationUnit,
    options: &Options,
    hints: &ResolutionHints,
) -> Vec<Model> {
    let mut out = Vec::new();
    collect_models_from_decls(unit.type_decls(), &mut out, options, hints);
    out
}

fn collect_models_from_decls(
    decls: &[TypeDecl],
    out: &mut Vec<Model>,
    options: &Options,
    hints: &ResolutionHints,
) {
    for decl in decls {
        match decl {
            TypeDecl::Class(class) => {
                out.push(model_from_class(class, options, hints));
                collect_models_from_class_body(&class.body.declarations, out, options, hints);
            }
            TypeDecl::Record(record) => {
                out.push(model_from_record(record, options, hints));
                collect_models_from_record_body(&record.body.members, out, options, hints);
            }
            TypeDecl::Enum(enum_decl) => {
                out.push(model_from_enum(enum_decl, options, hints));
                collect_models_from_class_body(&enum_decl.body.members, out, options, hints);
            }
            _ => {}
        }
    }
}

fn collect_models_from_class_body(
    decls: &[ClassBodyDecl],
    out: &mut Vec<Model>,
    options: &Options,
    hints: &ResolutionHints,
) {
    for decl in decls {
        match decl {
            ClassBodyDecl::Class(class) => {
                out.push(model_from_class(class, options, hints));
                collect_models_from_class_body(&class.body.declarations, out, options, hints);
            }
            ClassBodyDecl::Record(record) => {
                out.push(model_from_record(record, options, hints));
                collect_models_from_record_body(&record.body.members, out, options, hints);
            }
            ClassBodyDecl::Enum(enum_decl) => {
                out.push(model_from_enum(enum_decl, options, hints));
                collect_models_from_class_body(&enum_decl.body.members, out, options, hints);
            }
            _ => {}
        }
    }
}

fn collect_models_from_record_body(
    members: &[RecordBodyDecl],
    out: &mut Vec<Model>,
    options: &Options,
    hints: &ResolutionHints,
) {
    for member in members {
        match member {
            RecordBodyDecl::Class(class) => {
                out.push(model_from_class(class, options, hints));
                collect_models_from_class_body(&class.body.declarations, out, options, hints);
            }
            RecordBodyDecl::Record(record) => {
                out.push(model_from_record(record, options, hints));
                collect_models_from_record_body(&record.body.members, out, options, hints);
            }
            RecordBodyDecl::Enum(enum_decl) => {
                out.push(model_from_enum(enum_decl, options, hints));
                collect_models_from_class_body(&enum_decl.body.members, out, options, hints);
            }
            _ => {}
        }
    }
}

struct ModelIndex {
    models: HashMap<String, Option<Rc<Model>>>,
    attempted: HashSet<String>,
    options: Options,
}

impl ModelIndex {
    fn from_unit(unit: &CompilationUnit, options: &Options, hints: &ResolutionHints) -> Self {
        let mut index = Self {
            models: HashMap::new(),
            attempted: HashSet::new(),
            options: options.clone(),
        };
        for model in models_from_unit(unit, options, hints) {
            index.insert(model);
        }
        index
    }

    fn get(&self, qualified: &str) -> Option<Rc<Model>> {
        self.models.get(qualified).and_then(Option::as_ref).cloned()
    }

    fn insert(&mut self, model: Model) {
        let qualified = model.qualified_name.clone();
        match self.models.entry(qualified) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(Rc::new(model)));
            }
        }
    }

    fn insert_if_absent(&mut self, model: Model) {
        self.models
            .entry(model.qualified_name.clone())
            .or_insert_with(|| Some(Rc::new(model)));
    }

    fn get_or_resolve(
        &mut self,
        name: &str,
        hints: &ResolutionHints,
        resolve: &mut dyn FnMut(&str, &ResolutionHints) -> Option<String>,
    ) -> Option<Rc<Model>> {
        let qualified = hints.key(name);
        if let Some(model) = self.get(&qualified) {
            return Some(model);
        }
        if self.models.contains_key(&qualified) {
            return None;
        }
        if !self.attempted.insert(qualified.clone()) {
            return None;
        }

        let source = resolve(name, hints)?;
        let source_hints = resolution_hints(&source);
        if let Ok(unit) = parse(&source) {
            for model in models_from_unit(&unit, &self.options, &source_hints) {
                self.insert_if_absent(model);
            }
        }
        self.get(&source_hints.key(name))
    }
}

struct BuildContext<'a> {
    index: &'a mut ModelIndex,
    resolve: &'a mut dyn FnMut(&str, &ResolutionHints) -> Option<String>,
    options: &'a Options,
    now: &'a Now,
}

fn build(
    model: &Model,
    index: &mut ModelIndex,
    resolve: &mut dyn FnMut(&str, &ResolutionHints) -> Option<String>,
    options: &Options,
    now: &Now,
) -> Result<Generated, GenerateError> {
    let mut stack = Vec::new();
    let mut ctx = BuildContext {
        index,
        resolve,
        options,
        now,
    };
    let value = build_model(model, &mut ctx, &mut stack, 0);
    let field_count = value.as_object().map_or(0, Map::len);
    let json = to_json(&value, options.indent)?;
    Ok(Generated {
        class_name: model.name.clone(),
        field_count,
        json,
    })
}

fn to_json(value: &Value, indent: usize) -> Result<String, GenerateError> {
    let indent: &[u8] = if indent == 4 { b"    " } else { b"  " };
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|e| GenerateError::Failed(format!("Could not serialize JSON: {e}")))?;
    String::from_utf8(out)
        .map_err(|e| GenerateError::Failed(format!("Could not serialize JSON: {e}")))
}

fn build_model(
    model: &Model,
    ctx: &mut BuildContext<'_>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Value {
    if depth >= ctx.options.max_depth
        || stack
            .iter()
            .any(|seen| seen.as_str() == model.name.as_str())
    {
        return Value::Null;
    }

    stack.push(model.name.clone());
    let mut collected = Vec::new();
    let mut collect_stack = Vec::new();
    collect_fields(
        model,
        model.naming,
        ctx,
        &mut collect_stack,
        depth,
        &mut collected,
    );
    collected.retain(|field| !model.ignored_properties.contains(&field.java_name));

    let mut map = Map::new();
    for field in collected {
        if let Some(value) = value_for_type(&field.ty, &field.hints, ctx, stack, depth + 1) {
            map.insert(field.name, value);
        }
    }
    stack.pop();

    Value::Object(map)
}

fn collect_fields(
    model: &Model,
    root_naming: NamingStrategy,
    ctx: &mut BuildContext<'_>,
    stack: &mut Vec<String>,
    depth: usize,
    out: &mut Vec<CollectedField>,
) {
    if depth >= ctx.options.max_depth
        || stack
            .iter()
            .any(|seen| seen.as_str() == model.name.as_str())
    {
        return;
    }

    stack.push(model.name.clone());
    if ctx.options.include_inherited
        && let Some(parent) = &model.extends
        && let Some(parent_model) = model_for_type(parent, &model.hints, ctx)
        && !parent_model.is_enum
        && !parent_model.ignore_type
    {
        collect_fields(&parent_model, root_naming, ctx, stack, depth + 1, out);
    }

    for field in &model.fields {
        if model.ignored_properties.contains(&field.java_name) {
            continue;
        }
        let name = field
            .explicit_json_name
            .clone()
            .unwrap_or_else(|| root_naming.apply(&field.java_name));
        out.push(CollectedField {
            name,
            java_name: field.java_name.clone(),
            ty: field.ty.clone(),
            hints: model.hints.clone(),
        });
    }
    stack.pop();
}

fn model_for_type(
    ty: &TypeRef,
    hints: &ResolutionHints,
    ctx: &mut BuildContext<'_>,
) -> Option<Rc<Model>> {
    match ty {
        TypeRef::Named { name, .. } => ctx.index.get_or_resolve(name, hints, ctx.resolve),
        _ => None,
    }
}

fn unknown_value(options: &Options) -> Value {
    match options.unknown_type {
        UnknownType::Null => Value::Null,
        UnknownType::Object => Value::Object(Map::new()),
    }
}

fn unknown_collection(options: &Options) -> Value {
    match options.unknown_type {
        UnknownType::Null => Value::Array(Vec::new()),
        UnknownType::Object => Value::Array(vec![Value::Object(Map::new())]),
    }
}

fn value_for_type(
    ty: &TypeRef,
    hints: &ResolutionHints,
    ctx: &mut BuildContext<'_>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Option<Value> {
    match ty {
        TypeRef::Primitive(name) => {
            Some(defaults::default_scalar(name, ctx.now).unwrap_or(Value::Null))
        }
        TypeRef::Unknown => Some(unknown_value(ctx.options)),
        TypeRef::Array(inner) => Some(container_value(Some(inner), hints, ctx, stack, depth)),
        TypeRef::Named { name, args } => {
            if defaults::is_optional(name) {
                return Some(Value::Null);
            }
            if defaults::is_list(name) {
                return Some(container_value(args.first(), hints, ctx, stack, depth));
            }
            if defaults::is_map(name) {
                return Some(Value::Object(Map::new()));
            }
            if let Some(value) = defaults::default_scalar(name, ctx.now) {
                return Some(value);
            }
            if defaults::is_opaque(name) {
                return Some(unknown_value(ctx.options));
            }

            match ctx.index.get_or_resolve(name, hints, ctx.resolve) {
                Some(model) if model.ignore_type => None,
                Some(model) if model.is_enum => Some(
                    model
                        .enum_constants
                        .first()
                        .map_or(Value::Null, |constant| Value::String(constant.clone())),
                ),
                Some(model) => Some(build_model(&model, ctx, stack, depth)),
                None => Some(unknown_value(ctx.options)),
            }
        }
    }
}

fn container_value(
    element: Option<&TypeRef>,
    hints: &ResolutionHints,
    ctx: &mut BuildContext<'_>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Value {
    let Some(TypeRef::Named { name, .. }) = element else {
        return Value::Array(Vec::new());
    };

    if defaults::is_optional(name) || defaults::is_list(name) || defaults::is_map(name) {
        return Value::Array(Vec::new());
    }
    if defaults::is_opaque(name) {
        return unknown_collection(ctx.options);
    }
    if let Some(value) = defaults::default_scalar(name, ctx.now) {
        return if ctx.options.scalar_collection_sample {
            Value::Array(vec![value])
        } else {
            Value::Array(Vec::new())
        };
    }

    match ctx.index.get_or_resolve(name, hints, ctx.resolve) {
        Some(model) if model.ignore_type => Value::Array(Vec::new()),
        Some(model) if model.is_enum => {
            let value = model
                .enum_constants
                .first()
                .map_or(Value::Null, |constant| Value::String(constant.clone()));
            Value::Array(vec![value])
        }
        Some(model) => {
            let value = build_model(&model, ctx, stack, depth + 1);
            Value::Array(vec![value])
        }
        None => unknown_collection(ctx.options),
    }
}

fn has_annotation(modifiers: &[Modifier], name: &str) -> bool {
    annotations_from_modifiers(modifiers)
        .iter()
        .any(|annotation| annotation_name(annotation) == name)
}

fn has_transient(modifiers: &[Modifier]) -> bool {
    modifiers
        .iter()
        .any(|modifier| matches!(modifier, Modifier::Transient(_)))
}

fn class_naming(modifiers: &[Modifier], options: &Options) -> NamingStrategy {
    annotations_from_modifiers(modifiers)
        .iter()
        .find(|annotation| annotation_name(annotation) == "JsonNaming")
        .and_then(|annotation| naming_from_annotation(annotation))
        .unwrap_or(options.naming)
}

fn naming_from_annotation(annotation: &Annotation) -> Option<NamingStrategy> {
    match annotation_class_literal(annotation)?.as_str() {
        "SnakeCaseStrategy" | "SnakeCase" => Some(NamingStrategy::SnakeCase),
        "KebabCaseStrategy" | "KebabCase" => Some(NamingStrategy::KebabCase),
        "LowerCamelCaseStrategy" | "LowerCamelCase" => Some(NamingStrategy::CamelCase),
        "UpperCamelCaseStrategy" | "UpperCamelCase" | "PascalCase" => {
            Some(NamingStrategy::PascalCase)
        }
        "UpperSnakeCaseStrategy" | "UpperSnakeCase" => Some(NamingStrategy::UpperSnake),
        _ => None,
    }
}

fn annotation_class_literal(annotation: &Annotation) -> Option<String> {
    match annotation {
        Annotation::SingleElement { value, .. } => class_literal_name(value),
        Annotation::Normal { pairs, .. } => pairs
            .iter()
            .find(|pair| pair.key.name == "value")
            .and_then(|pair| class_literal_name(&pair.value)),
        Annotation::Marker { .. } => None,
    }
}

fn class_literal_name(value: &ElementValue) -> Option<String> {
    let ElementValue::Expr(Expr::ClassLit { type_expr, .. }) = value else {
        return None;
    };
    match type_expr.as_ref() {
        Type::Reference(ReferenceType::ClassOrInterfaceType(class)) => {
            Some(class.path.last_ident().name.clone())
        }
        _ => None,
    }
}

fn class_ignored_properties(modifiers: &[Modifier]) -> HashSet<String> {
    annotations_from_modifiers(modifiers)
        .iter()
        .find(|annotation| annotation_name(annotation) == "JsonIgnoreProperties")
        .map(|annotation| annotation_strings(annotation))
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn annotation_strings(annotation: &Annotation) -> Vec<String> {
    match annotation {
        Annotation::SingleElement { value, .. } => element_strings(value),
        Annotation::Normal { pairs, .. } => pairs
            .iter()
            .find(|pair| pair.key.name == "value")
            .map(|pair| element_strings(&pair.value))
            .unwrap_or_default(),
        Annotation::Marker { .. } => Vec::new(),
    }
}

fn element_strings(value: &ElementValue) -> Vec<String> {
    match value {
        ElementValue::Array { values, .. } => values.iter().filter_map(element_string).collect(),
        ElementValue::Expr(Expr::ArrayInit(array)) => {
            array.elements.iter().filter_map(expr_string).collect()
        }
        // Java annotations allow the braces of a single-element array to be
        // omitted: @JsonIgnoreProperties("name") and
        // @JsonIgnoreProperties(value = "name") both mean one property.
        ElementValue::Expr(expr) => expr_string(expr).into_iter().collect(),
        ElementValue::Annotation(_) => Vec::new(),
    }
}

fn json_access_is_write_only(annotations: &[&Annotation]) -> bool {
    annotations
        .iter()
        .filter(|annotation| annotation_name(annotation) == "JsonProperty")
        .any(|annotation| annotation_access(annotation).as_deref() == Some("WRITE_ONLY"))
}

fn annotation_access(annotation: &Annotation) -> Option<String> {
    match annotation {
        Annotation::Normal { pairs, .. } => pairs
            .iter()
            .find(|pair| pair.key.name == "access")
            .and_then(|pair| match &pair.value {
                ElementValue::Expr(expr) => expr_name(expr),
                _ => None,
            }),
        Annotation::SingleElement { .. } | Annotation::Marker { .. } => None,
    }
}

fn expr_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::FieldAccess(field) => Some(field.field.name.clone()),
        Expr::Ident(identifier) => Some(identifier.name.clone()),
        _ => None,
    }
}

fn has_static(modifiers: &[Modifier]) -> bool {
    modifiers
        .iter()
        .any(|modifier| matches!(modifier, Modifier::Static(_)))
}

fn annotations_from_modifiers(modifiers: &[Modifier]) -> Vec<&Annotation> {
    modifiers
        .iter()
        .filter_map(|modifier| match modifier {
            Modifier::Annotation(annotation) => Some(annotation),
            _ => None,
        })
        .collect()
}

fn is_json_ignored(annotations: &[&Annotation]) -> bool {
    annotations
        .iter()
        .find(|annotation| annotation_name(annotation) == "JsonIgnore")
        .is_some_and(|annotation| annotation_bool(annotation).unwrap_or(true))
}

fn json_property_name(annotations: &[&Annotation]) -> Option<String> {
    annotations
        .iter()
        .find(|annotation| annotation_name(annotation) == "JsonProperty")
        .and_then(|annotation| annotation_string(annotation))
        .filter(|name| !name.is_empty())
}

fn annotation_name(annotation: &Annotation) -> &str {
    annotation.path().last_ident().name.as_str()
}

fn annotation_string(annotation: &Annotation) -> Option<String> {
    match annotation {
        Annotation::SingleElement { value, .. } => element_string(value),
        Annotation::Normal { pairs, .. } => pairs
            .iter()
            .find(|pair| pair.key.name == "value")
            .and_then(|pair| element_string(&pair.value)),
        Annotation::Marker { .. } => None,
    }
}

fn annotation_bool(annotation: &Annotation) -> Option<bool> {
    match annotation {
        Annotation::Marker { .. } => Some(true),
        Annotation::SingleElement { value, .. } => element_bool(value),
        Annotation::Normal { pairs, .. } => pairs
            .iter()
            .find(|pair| pair.key.name == "value")
            .and_then(|pair| element_bool(&pair.value)),
    }
}

fn element_string(value: &ElementValue) -> Option<String> {
    match value {
        ElementValue::Expr(expr) => expr_string(expr),
        _ => None,
    }
}

fn element_bool(value: &ElementValue) -> Option<bool> {
    match value {
        ElementValue::Expr(expr) => expr_bool(expr),
        _ => None,
    }
}

fn expr_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(Lit::Str(value)) => Some(unescape_java_string(&value.value)),
        Expr::Paren { expr, .. } => expr_string(expr),
        _ => None,
    }
}

fn expr_bool(expr: &Expr) -> Option<bool> {
    match expr {
        Expr::Literal(Lit::Bool(value)) => Some(value.value),
        Expr::Paren { expr, .. } => expr_bool(expr),
        _ => None,
    }
}

/// The lexer keeps Java escape sequences verbatim; annotation values need
/// their runtime string (for example `@JsonProperty("a\"b")` → `a"b`).
fn unescape_java_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let mut hex = String::new();
                for _ in 0..4 {
                    if let Some(next) = chars.next() {
                        hex.push(next);
                    }
                }
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    out.push(ch);
                }
            }
            Some(first @ '0'..='7') => {
                let mut code = first.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.clone().next().and_then(|next| next.to_digit(8)) {
                        Some(digit) => {
                            chars.next();
                            code = code * 8 + digit;
                        }
                        None => break,
                    }
                }
                if let Some(ch) = char::from_u32(code) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(source: &str, needle: &str) -> Option<usize> {
        source.find(needle)
    }

    #[test]
    fn builds_basic_pojo_with_defaults() {
        let source = r#"
import java.time.LocalDateTime;
import java.util.List;
import java.util.Map;

public class User {
    private Long id;
    private String name;
    private boolean active;
    private List<String> tags;
    private Map<String, Object> attributes;
    private LocalDateTime createdAt;
    private Status status;
    private Address address;
    private static final long serialVersionUID = 1L;

    @com.fasterxml.jackson.annotation.JsonIgnore
    private String password;

    @com.fasterxml.jackson.annotation.JsonProperty("nick_name")
    private String nickName;

    public static class Address {
        private String city;
        private int zipCode;
    }

    public enum Status { ACTIVE, DISABLED }
}
"#;
        let generated = generate(source, caret(source, "private Long id")).unwrap();
        assert_eq!(generated.class_name, "User");
        assert_eq!(generated.field_count, 9);
        assert_eq!(
            generated.json,
            r#"{
  "id": 0,
  "name": "",
  "active": false,
  "tags": [],
  "attributes": {},
  "createdAt": "1970-01-01 00:00:00",
  "status": "ACTIVE",
  "address": {
    "city": "",
    "zipCode": 0
  },
  "nick_name": ""
}"#
        );
    }

    #[test]
    fn builds_record_components() {
        let source = "public record Point(int x, int y, List<String> labels) {}";
        let generated = generate(source, caret(source, "record")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "x": 0,
  "y": 0,
  "labels": []
}"#
        );
    }

    #[test]
    fn picks_the_innermost_class() {
        let source = r#"
public class Outer {
    private int outer;

    public static class Inner {
        private String inner;
    }
}
"#;
        let generated = generate(source, caret(source, "private String inner")).unwrap();
        assert_eq!(generated.class_name, "Inner");
        assert_eq!(generated.json, "{\n  \"inner\": \"\"\n}");
    }

    #[test]
    fn handles_ignore_and_property_annotations() {
        let source = r#"
public class Annotated {
    @JsonIgnore
    private String hidden;
    @JsonIgnore(false)
    private String visible;
    @JsonProperty(value = "renamed")
    private String original;
}
"#;
        let generated = generate(source, caret(source, "private String hidden")).unwrap();
        assert_eq!(
            generated.json,
            "{\n  \"visible\": \"\",\n  \"renamed\": \"\"\n}"
        );
    }

    #[test]
    fn skips_static_fields_and_expands_multiple_declarators() {
        let source = r#"
public class Statics {
    private static int counter;
    private int a, b;
    private static final long serialVersionUID = 1L;
}
"#;
        let generated = generate(source, caret(source, "private static int counter")).unwrap();
        assert_eq!(generated.json, "{\n  \"a\": 0,\n  \"b\": 0\n}");
    }

    #[test]
    fn unknown_external_and_recursive_types_become_null() {
        let source = r#"
public class Node {
    private Node next;
    private External external;
    private int value;
}
"#;
        let generated = generate(source, caret(source, "private Node next")).unwrap();
        assert_eq!(
            generated.json,
            "{\n  \"next\": null,\n  \"external\": null,\n  \"value\": 0\n}"
        );
    }

    #[test]
    fn collection_map_optional_array_and_date_defaults() {
        let source = r#"
public class Kinds {
    private byte[] bytes;
    private String[] names;
    private Optional<String> maybe;
    private Map<String, Integer> counts;
    private Set<Long> ids;
    private UUID uuid;
    private LocalDate day;
    private Instant instant;
    private Character letter;
    private Double ratio;
}
"#;
        let generated = generate(source, caret(source, "private byte[] bytes")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "bytes": [],
  "names": [],
  "maybe": null,
  "counts": {},
  "ids": [],
  "uuid": "00000000-0000-0000-0000-000000000000",
  "day": "1970-01-01",
  "instant": "1970-01-01 00:00:00",
  "letter": "",
  "ratio": 0.0
}"#
        );
    }

    #[test]
    fn formats_all_date_and_time_defaults() {
        let source = r#"
public class Dates {
    private LocalDate date;
    private LocalTime time;
    private LocalDateTime dateTime;
    private Date legacyDate;
    private Timestamp timestamp;
    private Instant instant;
    private ZonedDateTime zoned;
    private OffsetDateTime offset;
}
"#;
        let generated = generate(source, Some(0)).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "date": "1970-01-01",
  "time": "00:00:00",
  "dateTime": "1970-01-01 00:00:00",
  "legacyDate": "1970-01-01 00:00:00",
  "timestamp": "1970-01-01 00:00:00",
  "instant": "1970-01-01 00:00:00",
  "zoned": "1970-01-01 00:00:00",
  "offset": "1970-01-01 00:00:00"
}"#
        );
    }

    #[test]
    fn expands_list_of_same_file_pojos() {
        let source = r#"
public class A {
    private Long id;
    private List<UploadFileVo> fileList;
}

class UploadFileVo {
    private String filePath;
    private String fileName;
    private String newFileName;
}
"#;
        let generated = generate(source, caret(source, "private Long id")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "id": 0,
  "fileList": [
    {
      "filePath": "",
      "fileName": "",
      "newFileName": ""
    }
  ]
}"#
        );
    }

    #[test]
    fn expands_list_of_external_pojos_through_the_resolver() {
        let source = "public class A { private Long id; private List<UploadFileVo> fileList; }";
        let external = r#"
public class UploadFileVo {
    private String filePath;
    private String fileName;
    private String newFileName;
}
"#;
        let mut resolve = |name: &str, _hints: &ResolutionHints| {
            (name == "UploadFileVo").then(|| external.to_string())
        };
        let generated = generate_with(
            source,
            Some(0),
            &mut resolve,
            &Options::default(),
            &Now::epoch(),
        )
        .unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "id": 0,
  "fileList": [
    {
      "filePath": "",
      "fileName": "",
      "newFileName": ""
    }
  ]
}"#
        );
    }

    #[test]
    fn unresolved_list_elements_stay_empty() {
        let source = "public class A { private List<External> items; }";
        let generated = generate(source, Some(0)).unwrap();
        assert_eq!(generated.json, "{\n  \"items\": []\n}");
    }

    #[test]
    fn expands_arrays_and_enum_lists() {
        let source = r#"
public class A {
    private Address[] addresses;
    private List<Status> statuses;
}

class Address {
    private String city;
}

enum Status { ON, OFF }
"#;
        let generated = generate(source, caret(source, "private Address[] addresses")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "addresses": [
    {
      "city": ""
    }
  ],
  "statuses": [
    "ON"
  ]
}"#
        );
    }

    fn generate_with_options(
        source: &str,
        offset: Option<usize>,
        options: &Options,
    ) -> Result<Generated, GenerateError> {
        generate_with(source, offset, &mut |_, _| None, options, &Now::epoch())
    }

    #[test]
    fn includes_parent_fields_before_child_fields() {
        let source = r#"
public class Child extends Parent {
    private String child;
}

class Parent {
    private int id;
    private String name;
}
"#;
        let generated = generate(source, caret(source, "public class Child")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "id": 0,
  "name": "",
  "child": ""
}"#
        );

        let options = Options {
            include_inherited: false,
            ..Options::default()
        };
        let generated =
            generate_with_options(source, caret(source, "public class Child"), &options).unwrap();
        assert_eq!(generated.json, "{\n  \"child\": \"\"\n}");
    }

    #[test]
    fn lets_child_fields_override_parent_fields() {
        let source = r#"
public class Child extends Parent {
    private int name;
}
class Parent {
    private String name;
}
"#;
        let generated = generate(source, caret(source, "public class Child")).unwrap();
        assert_eq!(generated.json, "{\n  \"name\": 0\n}");
    }

    #[test]
    fn applies_json_naming_and_explicit_property_names() {
        let source = r#"
@JsonNaming(PropertyNamingStrategies.SnakeCaseStrategy.class)
public class Naming {
    private String filePath;
    @JsonProperty("explicitName")
    private String otherName;
}
"#;
        let generated = generate(source, caret(source, "public class Naming")).unwrap();
        assert_eq!(
            generated.json,
            "{\n  \"file_path\": \"\",\n  \"explicitName\": \"\"\n}"
        );
    }

    #[test]
    fn applies_json_ignore_properties() {
        let source = r#"
@JsonIgnoreProperties({"secret", "hidden"})
public class Filtered {
    private String secret;
    private String hidden;
    private String keep;
}
"#;
        let generated = generate(source, caret(source, "public class Filtered")).unwrap();
        assert_eq!(generated.json, "{\n  \"keep\": \"\"\n}");
    }

    #[test]
    fn skips_write_only_properties_and_keeps_read_only() {
        let source = r#"
public class Access {
    @JsonProperty(access = JsonProperty.Access.WRITE_ONLY)
    private String password;
    @JsonProperty(access = JsonProperty.Access.READ_ONLY)
    private String id;
}
"#;
        let generated = generate(source, caret(source, "public class Access")).unwrap();
        assert_eq!(generated.json, "{\n  \"id\": \"\"\n}");
    }

    #[test]
    fn omits_ignore_type_fields() {
        let source = r#"
public class Holder {
    private Hidden hidden;
    private String keep;

    @JsonIgnoreType
    public static class Hidden {
        private String value;
    }
}
"#;
        let generated = generate(source, caret(source, "private Hidden hidden")).unwrap();
        assert_eq!(generated.json, "{\n  \"keep\": \"\"\n}");
    }

    #[test]
    fn applies_options_for_static_transient_unknown_and_collections() {
        let source = r#"
public class Opt {
    private String filePath;
    private static int counter;
    private transient String temp;
    private External ext;
    private List<String> tags;
}
"#;
        let options = Options {
            naming: NamingStrategy::SnakeCase,
            include_static: true,
            include_transient: false,
            unknown_type: UnknownType::Object,
            scalar_collection_sample: true,
            indent: 4,
            ..Options::default()
        };
        let generated = generate_with_options(source, Some(0), &options).unwrap();
        assert_eq!(
            generated.json,
            r#"{
    "file_path": "",
    "counter": 0,
    "ext": {},
    "tags": [
        ""
    ]
}"#
        );
    }

    #[test]
    fn honours_max_depth() {
        let source = r#"
public class A {
    private B b;
}
class B {
    private String value;
}
"#;
        let options = Options {
            max_depth: 1,
            ..Options::default()
        };
        let generated =
            generate_with_options(source, caret(source, "public class A"), &options).unwrap();
        assert_eq!(generated.json, "{\n  \"b\": null\n}");
    }

    #[test]
    fn extracts_wildcard_imports() {
        let hints = resolution_hints("package com.use;\nimport com.b.*;\npublic class A {}\n");
        assert_eq!(hints.wildcard_packages, vec!["com.b".to_string()]);
    }

    #[test]
    fn extracts_resolution_hints() {
        let source = r#"
package com.use;
import com.b.UploadFileVo;
public class A {
    private UploadFileVo file;
}
"#;
        let hints = resolution_hints(source);
        assert_eq!(hints.package.as_deref(), Some("com.use"));
        assert_eq!(
            hints.imports.get("UploadFileVo").map(String::as_str),
            Some("com.b.UploadFileVo")
        );
        assert_eq!(
            hints.qualified("UploadFileVo").as_deref(),
            Some("com.b.UploadFileVo")
        );
        assert_eq!(hints.qualified("Other").as_deref(), Some("com.use.Other"));
    }

    #[test]
    fn applies_single_string_ignore_properties() {
        for annotation in [
            "@JsonIgnoreProperties(\"secret\")",
            "@JsonIgnoreProperties(value = \"secret\")",
        ] {
            let source = format!(
                "{annotation}\npublic class Single {{\n    private String secret;\n    private String keep;\n}}\n"
            );
            let generated = generate(&source, caret(&source, "public class Single")).unwrap();
            assert_eq!(generated.json, "{\n  \"keep\": \"\"\n}", "{annotation}");
        }
    }

    #[test]
    fn child_ignore_properties_filters_inherited_fields() {
        let source = r#"
class Parent {
    private String secret;
    private String keep;
}

@JsonIgnoreProperties({"secret"})
public class Child extends Parent {
    private String own;
}
"#;
        let generated = generate(source, caret(source, "public class Child")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "keep": "",
  "own": ""
}"#
        );
    }

    #[test]
    fn child_json_naming_renames_inherited_fields() {
        let source = r#"
class Parent {
    private String filePath;
}

@JsonNaming(PropertyNamingStrategies.SnakeCaseStrategy.class)
public class Child extends Parent {
    private String childName;
}
"#;
        let generated = generate(source, caret(source, "public class Child")).unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "file_path": "",
  "child_name": ""
}"#
        );
    }

    #[test]
    fn ignore_properties_matches_before_naming() {
        let source = r#"
@JsonNaming(PropertyNamingStrategies.SnakeCaseStrategy.class)
@JsonIgnoreProperties({"filePath"})
public class Filtered {
    private String filePath;
    private String otherName;
}
"#;
        let generated = generate(source, caret(source, "public class Filtered")).unwrap();
        assert_eq!(generated.json, "{\n  \"other_name\": \"\"\n}");
    }

    #[test]
    fn omits_ignore_type_fields_and_list_elements() {
        let source = r#"
public class Holder {
    private Hidden hidden;
    private List<Hidden> hiddenList;
    private String keep;

    @JsonIgnoreType
    public static class Hidden {
        private String value;
    }
}
"#;
        let generated = generate(source, caret(source, "private Hidden hidden")).unwrap();
        assert_eq!(
            generated.json,
            "{\n  \"hiddenList\": [],\n  \"keep\": \"\"\n}"
        );
    }

    #[test]
    fn resolves_nested_external_types_with_their_own_imports() {
        let source = r#"
package com.a;
import com.b.UploadFileVo;
public class A {
    private UploadFileVo file;
}
"#;
        let upload = r#"
package com.b;
import com.c.Address;
public class UploadFileVo {
    private Address address;
}
"#;
        let address = r#"
package com.c;
public class Address {
    private String cOnly;
}
"#;
        let wrong_address = r#"
package com.a;
public class Address {
    private String aOnly;
}
"#;
        let mut resolve = |name: &str, hints: &ResolutionHints| match name {
            "UploadFileVo" => Some(upload.to_string()),
            "Address" if hints.qualified("Address").as_deref() == Some("com.c.Address") => {
                Some(address.to_string())
            }
            "Address" => Some(wrong_address.to_string()),
            _ => None,
        };
        let generated = generate_with(
            source,
            caret(source, "public class A"),
            &mut resolve,
            &Options::default(),
            &Now::epoch(),
        )
        .unwrap();
        assert_eq!(
            generated.json,
            r#"{
  "file": {
    "address": {
      "cOnly": ""
    }
  }
}"#
        );
    }

    #[test]
    fn opaque_jdk_types_do_not_use_the_resolver() {
        let source = "public class Opaque { private Object object; private Class<?> clazz; }";
        let wrong_object = "public class Object { private String wrong; }";
        let mut resolve = |name: &str, _hints: &ResolutionHints| {
            (name == "Object").then(|| wrong_object.to_string())
        };
        let generated = generate_with(
            source,
            Some(0),
            &mut resolve,
            &Options::default(),
            &Now::epoch(),
        )
        .unwrap();
        assert_eq!(
            generated.json,
            "{\n  \"object\": null,\n  \"clazz\": null\n}"
        );

        let options = Options {
            unknown_type: UnknownType::Object,
            ..Options::default()
        };
        let generated = generate_with_options(source, Some(0), &options).unwrap();
        assert_eq!(generated.json, "{\n  \"object\": {},\n  \"clazz\": {}\n}");
    }

    #[test]
    fn reports_parse_errors_with_a_location() {
        let source = "public class Broken { private String value }";
        let error = generate(source, Some(0)).unwrap_err();
        match error {
            GenerateError::Failed(message) => {
                assert!(message.contains("Java parse error"), "{message}");
                assert!(message.contains("line 1"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn asks_for_a_choice_between_top_level_classes() {
        let source = "class A { int a; }\nclass B { int b; }\n";
        let error = generate(source, Some(source.len())).unwrap_err();
        match error {
            GenerateError::NeedChoice(names) => {
                assert_eq!(names, vec!["A".to_string(), "B".to_string()]);
            }
            other => panic!("expected a choice, got {other:?}"),
        }

        let generated = generate_named(source, "B").unwrap();
        assert_eq!(generated.json, "{\n  \"b\": 0\n}");
    }

    #[test]
    fn handles_empty_classes() {
        let source = "public class Empty {}";
        let generated = generate(source, Some(0)).unwrap();
        assert_eq!(generated.field_count, 0);
        assert_eq!(generated.json, "{}");
    }

    #[test]
    fn escapes_property_names() {
        let source = r#"
public class Escaped {
    @JsonProperty("a\"b")
    private String value;
}
"#;
        let generated = generate(source, caret(source, "private String value")).unwrap();
        assert_eq!(generated.json, "{\n  \"a\\\"b\": \"\"\n}");
    }

    #[test]
    fn supports_record_inside_class_and_varargs_components() {
        let source = r#"
public class Holder {
    public record Entry(String key, Object... values) {}
}
"#;
        let generated = generate(source, caret(source, "public record Entry")).unwrap();
        assert_eq!(generated.class_name, "Entry");
        assert_eq!(generated.json, "{\n  \"key\": \"\",\n  \"values\": []\n}");
    }
}
