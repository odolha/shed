//! Makes the nexus API's TypeScript and Kotlin types from `crates/shed-api`,
//! with typeshare's parser and language generators, plus two adjustments so
//! the types describe the JSON serde actually writes:
//!
//! - TypeScript: an `Option` field is `name?: T | null` (serde writes `null`
//!   for `None`; typeshare assumes it is left out), and an enum without data
//!   is a union of string literals rather than a TypeScript `enum` (so JSON
//!   literals type-check, and it is erasable syntax).
//! - Kotlin: in a sealed class, a variant's content type that has the name of
//!   one of the class's own variants (`Event.Screen(content: Screen)`) is
//!   written with its package, or Kotlin would take it for the nested class.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use typeshare_core::context::{ParseContext, ParseFileContext};
use typeshare_core::language::{
    CrateTypes, Kotlin, Language, SINGLE_FILE_CRATE_NAME, ScopedCrateTypes, SupportedLanguage,
    TypeScript,
};
use typeshare_core::parser::{ParsedData, parse};
use typeshare_core::reconcile::reconcile_aliases;
use typeshare_core::rust_types::{
    FieldDecorator, RustConst, RustConstExpr, RustEnum, RustStruct, RustType, RustTypeAlias, RustTypeFormatError,
    SpecialRustType,
};

/// The Kotlin package of the generated types.
pub const KOTLIN_PACKAGE: &str = "dev.shed.api";

/// The generated files, relative to the workspace root.
pub const TYPESCRIPT_FILE: &str = "web/src/api/gen/api.ts";
pub const KOTLIN_FILE: &str = "android/app/src/main/java/dev/shed/api/Api.kt";

/// The workspace root (this crate is `crates/shed-codegen`).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Parses every `#[typeshare]` type in `crates/shed-api/src`, as one namespace.
fn parse_api(root: &Path) -> Result<ParsedData> {
    let src = root.join("crates/shed-api/src");
    let mut files: Vec<PathBuf> = fs::read_dir(&src)
        .with_context(|| format!("reading {}", src.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<_>>()?;
    files.retain(|p| p.extension().is_some_and(|e| e == "rs"));
    files.sort();

    let context = ParseContext::default();
    let mut all = ParsedData::new(SINGLE_FILE_CRATE_NAME, String::new(), false);
    for path in files {
        let source_code = fs::read_to_string(&path)?;
        let parsed = parse(
            &context,
            ParseFileContext {
                source_code,
                crate_name: SINGLE_FILE_CRATE_NAME,
                file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                file_path: path.clone(),
            },
        )
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if let Some(parsed) = parsed {
            all += parsed;
        }
    }
    if !all.errors.is_empty() {
        let errors: Vec<String> =
            all.errors.iter().map(|e| format!("{}: {}", e.file_name, e.error)).collect();
        bail!("typeshare could not read shed-api:\n{}", errors.join("\n"));
    }

    // One namespace: two types with one name would be declared twice.
    let mut seen = BTreeSet::new();
    let names = all
        .structs
        .iter()
        .map(|s| &s.id.renamed)
        .chain(all.enums.iter().map(|e| &e.shared().id.renamed))
        .chain(all.aliases.iter().map(|a| &a.id.renamed));
    for name in names {
        if !seen.insert(name.clone()) {
            bail!("two shed-api types are called `{name}`; the generated files need one name each");
        }
    }

    // Sorts the types, for output that does not depend on the parse order.
    let mut by_crate = BTreeMap::from([(SINGLE_FILE_CRATE_NAME, all)]);
    reconcile_aliases(&mut by_crate);
    Ok(by_crate.remove(&SINGLE_FILE_CRATE_NAME).unwrap())
}

fn generate(lang: &mut dyn Language, data: ParsedData) -> Result<String> {
    let mut out = Vec::new();
    lang.generate_types(&mut out, &CrateTypes::new(), data)?;
    // typeshare leaves spaces at some line ends (`export type Call = `).
    let text = String::from_utf8(out)?;
    let lines: Vec<&str> = text.trim_end().lines().map(str::trim_end).collect();
    Ok(lines.join("\n") + "\n")
}

/// `api.ts`, whole.
pub fn typescript(root: &Path) -> Result<String> {
    let mut lang = Ts(TypeScript { no_version_header: true, ..Default::default() });
    let body = generate(&mut lang, parse_api(root)?)?;
    Ok(format!(
        "// Generated from crates/shed-api by shed-codegen (typeshare): do not edit.\n\
         // Run `cargo run -p shed-codegen` after changing shed-api.\n\
         //\n\
         // The JSON is serde's: enums with data are `{{ type, content }}`, enums\n\
         // without are strings; a `null` field and a missing one both mean none.\n\
         \n\
         export const API_VERSION = {};\n\
         export const MIN_API_VERSION = {};\n\
         \n\
         {body}",
        shed_api::API_VERSION,
        shed_api::MIN_API_VERSION,
    ))
}

/// `Api.kt`, whole.
pub fn kotlin(root: &Path) -> Result<String> {
    let mut lang = Kt(Kotlin {
        package: KOTLIN_PACKAGE.into(),
        no_version_header: true,
        ..Default::default()
    });
    let body = generate(&mut lang, parse_api(root)?)?;
    Ok(format!(
        "{KOTLIN_HEADER}\n{body}\n\
         const val API_VERSION: UInt = {}u\n\
         const val MIN_API_VERSION: UInt = {}u\n",
        shed_api::API_VERSION,
        shed_api::MIN_API_VERSION,
    ))
}

const KOTLIN_HEADER: &str = r#"// Generated from crates/shed-api by shed-codegen (typeshare): do not edit.
// Run `cargo run -p shed-codegen` after changing shed-api.
//
// The JSON is serde's. Read and write it with:
//
//     val ShedJson = Json {
//         classDiscriminator = "type"  // the default, and serde's tag
//         ignoreUnknownKeys = true     // newer peers add fields
//         coerceInputValues = true     // and enum values (below)
//         serializersModule = …        // and variants (below)
//     }
//
// - Enums with data are sealed classes: {"type": "V", "content": …} is
//   `data class V(val content: T)`, and {"type": "V"} (no data) an `object V`.
//   Encode one through its base type (`ShedJson.encodeToString<Call>(call)`),
//   or the "type" is not written.
// - Enums without data are enum classes over camelCase strings.
// - Fields serde gives a default are nullable here, null when left out. Keep
//   `encodeDefaults` off (the default): serde reads a missing field as its
//   default, but null only for an Option, not for a String, Boolean or List.
//   (With `encodeDefaults = true`, also set `explicitNulls = false`.)
// - u32 is UInt; f32 Float; f64 Double.
// - Newer peers may send what this version does not know. `AiEntry` and
//   `Event` have an `Unknown` variant for that (never sent): make it their
//   default deserializer, and set `coerceInputValues = true`
//   so an unknown value of an enum without data, in a field with a default,
//   reads as null. Elsewhere an unknown variant or value fails to decode:
//
//     serializersModule = SerializersModule {
//         polymorphicDefaultDeserializer(AiEntry::class) { AiEntry.Unknown.serializer() }
//         polymorphicDefaultDeserializer(Event::class) { Event.Unknown.serializer() }
//     }
"#;

/// Writes both files under `root`; returns those that changed.
pub fn write_all(root: &Path) -> Result<Vec<&'static str>> {
    let mut changed = Vec::new();
    for (file, text) in [(TYPESCRIPT_FILE, typescript(root)?), (KOTLIN_FILE, kotlin(root)?)] {
        let path = root.join(file);
        if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        changed.push(file);
    }
    Ok(changed)
}

// --------------------------------------------------------------- TypeScript

/// typeshare's TypeScript, but with `| null` on `Option` fields and string
/// unions for enums without data.
struct Ts(TypeScript);

impl Language for Ts {
    fn type_map(&mut self) -> &HashMap<String, String> {
        self.0.type_map()
    }

    fn format_simple_type(
        &mut self,
        base: &String,
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_simple_type(base, generic_types)
    }

    fn format_generic_type(
        &mut self,
        base: &String,
        parameters: &[RustType],
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_generic_type(base, parameters, generic_types)
    }

    fn format_special_type(
        &mut self,
        special_ty: &SpecialRustType,
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_special_type(special_ty, generic_types)
    }

    fn begin_file(&mut self, w: &mut dyn Write, data: &ParsedData) -> io::Result<()> {
        self.0.begin_file(w, data)
    }

    fn write_imports(
        &mut self,
        w: &mut dyn Write,
        imports: ScopedCrateTypes<'_>,
    ) -> io::Result<()> {
        self.0.write_imports(w, imports)
    }

    fn end_file(&mut self, w: &mut dyn Write) -> io::Result<()> {
        self.0.end_file(w)
    }

    fn write_type_alias(&mut self, w: &mut dyn Write, t: &RustTypeAlias) -> io::Result<()> {
        self.0.write_type_alias(w, t)
    }

    fn write_const(&mut self, w: &mut dyn Write, c: &RustConst) -> io::Result<()> {
        self.0.write_const(w, c)
    }

    fn write_struct(&mut self, w: &mut dyn Write, rs: &RustStruct) -> io::Result<()> {
        // serde writes `None` as `null`: say so, through typeshare's per-field
        // type override (the `?` stays, as serde also reads a missing field).
        let mut rs = rs.clone();
        for field in &mut rs.fields {
            if let RustType::Special(SpecialRustType::Option(inner)) = &field.ty {
                let ty = self.0.format_type(inner, &rs.generic_types).map_err(io::Error::other)?;
                field
                    .decorators
                    .entry(SupportedLanguage::TypeScript)
                    .or_default()
                    .insert(FieldDecorator::NameValue("type".into(), format!("{ty} | null")));
            }
        }
        self.0.write_struct(w, &rs)
    }

    fn write_enum(&mut self, w: &mut dyn Write, e: &RustEnum) -> io::Result<()> {
        let RustEnum::Unit(shared) = e else {
            return self.0.write_enum(w, e);
        };
        ts_comments(w, "", &shared.comments)?;
        writeln!(w, "export type {} =", shared.id.renamed)?;
        let last = shared.variants.len().saturating_sub(1);
        for (i, v) in shared.variants.iter().enumerate() {
            ts_comments(w, "\t", &v.shared().comments)?;
            let end = if i == last { ";" } else { "" };
            writeln!(w, "\t| {:?}{end}", v.shared().id.renamed)?;
        }
        writeln!(w)
    }

    fn ignored_reference_types(&self) -> Vec<&str> {
        self.0.ignored_reference_types()
    }
}

fn ts_comments(w: &mut dyn Write, indent: &str, comments: &[String]) -> io::Result<()> {
    match comments {
        [] => Ok(()),
        [one] => writeln!(w, "{indent}/** {one} */"),
        many => {
            writeln!(w, "{indent}/**")?;
            for line in many {
                writeln!(w, "{indent} * {line}")?;
            }
            writeln!(w, "{indent} */")
        }
    }
}

// ------------------------------------------------------------------- Kotlin

/// typeshare's Kotlin, with content types that a sealed class's own variants
/// would shadow written with their package.
struct Kt(Kotlin);

impl Language for Kt {
    fn type_map(&mut self) -> &HashMap<String, String> {
        self.0.type_map()
    }

    fn format_simple_type(
        &mut self,
        base: &String,
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_simple_type(base, generic_types)
    }

    fn format_generic_type(
        &mut self,
        base: &String,
        parameters: &[RustType],
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_generic_type(base, parameters, generic_types)
    }

    fn format_special_type(
        &mut self,
        special_ty: &SpecialRustType,
        generic_types: &[String],
    ) -> Result<String, RustTypeFormatError> {
        self.0.format_special_type(special_ty, generic_types)
    }

    fn begin_file(&mut self, w: &mut dyn Write, data: &ParsedData) -> io::Result<()> {
        self.0.begin_file(w, data)
    }

    fn write_imports(
        &mut self,
        w: &mut dyn Write,
        imports: ScopedCrateTypes<'_>,
    ) -> io::Result<()> {
        self.0.write_imports(w, imports)
    }

    fn end_file(&mut self, w: &mut dyn Write) -> io::Result<()> {
        self.0.end_file(w)
    }

    fn write_type_alias(&mut self, w: &mut dyn Write, t: &RustTypeAlias) -> io::Result<()> {
        self.0.write_type_alias(w, t)
    }

    fn write_const(&mut self, w: &mut dyn Write, c: &RustConst) -> io::Result<()> {
        // typeshare has no Kotlin constants yet.
        let RustConstExpr::Int(value) = c.expr;
        let (ty, suffix) = match &c.r#type {
            RustType::Special(SpecialRustType::U32) => ("UInt", "u"),
            RustType::Special(SpecialRustType::I32) => ("Int", ""),
            other => return Err(io::Error::other(format!("a constant of type {other:?}"))),
        };
        writeln!(w, "const val {}: {ty} = {value}{suffix}\n", c.id.original)
    }

    fn write_struct(&mut self, w: &mut dyn Write, rs: &RustStruct) -> io::Result<()> {
        self.0.write_struct(w, rs)
    }

    fn write_enum(&mut self, w: &mut dyn Write, e: &RustEnum) -> io::Result<()> {
        // Inside `sealed class Event { … }`, `Screen` is `Event.Screen`: map
        // each variant's name to the package-level type while writing it.
        let shadowed: Vec<String> = match e {
            RustEnum::Unit(_) => Vec::new(),
            RustEnum::Algebraic { shared, .. } => {
                shared.variants.iter().map(|v| v.shared().id.original.clone()).collect()
            }
        };
        for name in &shadowed {
            self.0.type_mappings.insert(name.clone(), format!("{KOTLIN_PACKAGE}.{name}"));
        }
        let written = self.0.write_enum(w, e);
        for name in &shadowed {
            self.0.type_mappings.remove(name);
        }
        written
    }

    fn ignored_reference_types(&self) -> Vec<&str> {
        self.0.ignored_reference_types()
    }
}
