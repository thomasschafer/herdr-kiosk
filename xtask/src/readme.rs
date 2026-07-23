use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use herdr_kiosk::config::{Config, keys::default_keys_toml};
use syn::{
    Attribute, Field, Fields, GenericArgument, Item, ItemEnum, ItemMod, ItemStruct, Meta,
    PathArguments, Type, parse_file,
};

const CONFIG_START: &str = "<!-- CONFIG:START -->";
const CONFIG_END: &str = "<!-- CONFIG:END -->";

pub fn generate(readme_path: &Path, config_path: &Path, check: bool) -> Result<()> {
    let raw = fs::read_to_string(readme_path)
        .with_context(|| format!("failed to read {}", readme_path.display()))?;
    let current = normalize_line_endings(&raw);
    let generated = generate_config_reference(&current, config_path)?;

    if generated == current && (check || generated == raw) {
        println!("{} is up to date", readme_path.display());
        return Ok(());
    }
    if check {
        bail!(
            "{} is out of date; run `cargo xtask readme`",
            readme_path.display()
        );
    }

    fs::write(readme_path, generated)
        .with_context(|| format!("failed to write {}", readme_path.display()))?;
    println!("updated {}", readme_path.display());
    Ok(())
}

fn normalize_line_endings(input: &str) -> String {
    input.replace('\r', "")
}

fn generate_config_reference(readme: &str, config_path: &Path) -> Result<String> {
    let model = ConfigModel::parse(config_path)?;
    let config = model
        .structs
        .get("Config")
        .context("Config struct not found in the config module")?;
    let defaults = toml::Value::try_from(Config::default())
        .context("failed to serialize Config::default() as TOML")?;
    let mut docs = String::new();

    push_doc(&mut docs, &required_doc(&config.attrs, "Config")?);
    for field in named_fields(config)? {
        render_config_field(&mut docs, field, &model, &defaults)?;
    }

    replace_generated_section(readme, &docs)
}

fn render_config_field(
    docs: &mut String,
    field: &Field,
    model: &ConfigModel,
    defaults: &toml::Value,
) -> Result<()> {
    let name = field_name(field)?;
    let field_doc = required_doc(&field.attrs, &format!("Config::{name}"))?;
    let default = defaults.get(&name);

    if let Some(type_name) = direct_type_name(&field.ty)
        && let Some(nested) = model.structs.get(&type_name)
    {
        writeln!(docs, "### `[{name}]`\n")?;
        push_doc(docs, &field_doc);

        match type_name.as_str() {
            "ThemeConfig" => render_theme(docs, nested, model, default)?,
            "KeysConfig" => render_keys(docs),
            _ => {
                push_distinct_doc(docs, &field_doc, &required_doc(&nested.attrs, &type_name)?);
                render_curated_example(docs, &name);
                render_struct_fields(docs, nested, model, default)?;
            }
        }
        return Ok(());
    }

    writeln!(docs, "#### `{name}`\n")?;
    push_doc(docs, &field_doc);
    render_curated_example(docs, &name);
    render_collection_schema(docs, field, model)?;
    render_default(docs, default)?;
    Ok(())
}

fn render_curated_example(docs: &mut String, name: &str) {
    match name {
        "search_dirs" => docs.push_str(
            "Example:\n\n```toml\ninclude_non_git = false\nsearch_dirs = [\n  \"~/Code\",\n  { path = \"~/Work\", depth = 2, include_non_git = true },\n]\n```\n\n",
        ),
        "on_open" => docs.push_str(
            "Example:\n\n```toml\n[on_open]\npanes = [\n  { command = \"hx\", direction = \"right\" },\n]\n```\n\n",
        ),
        _ => {}
    }
}

fn render_struct_fields(
    docs: &mut String,
    item: &ItemStruct,
    model: &ConfigModel,
    defaults: Option<&toml::Value>,
) -> Result<()> {
    for field in named_fields(item)? {
        let name = field_name(field)?;
        writeln!(docs, "#### `{name}`\n")?;
        push_doc(
            docs,
            &required_doc(&field.attrs, &format!("{}::{name}", item.ident))?,
        );
        render_collection_schema(docs, field, model)?;
        render_default(docs, defaults.and_then(|value| value.get(&name)))?;
    }
    Ok(())
}

fn render_collection_schema(docs: &mut String, field: &Field, model: &ConfigModel) -> Result<()> {
    let Some(element_name) = collection_element_name(&field.ty) else {
        return Ok(());
    };

    if let Some(item) = model.structs.get(&element_name) {
        push_doc(docs, &required_doc(&item.attrs, &element_name)?);
        docs.push_str("Each entry is an inline table with:\n\n");
        for nested in named_fields(item)? {
            let name = field_name(nested)?;
            let prose = required_doc(&nested.attrs, &format!("{element_name}::{name}"))?;
            writeln!(docs, "- `{name}` — {}", prose.replace('\n', " "))?;
        }
        docs.push('\n');
    } else if let Some(item) = model.enums.get(&element_name) {
        push_doc(docs, &required_doc(&item.attrs, &element_name)?);
        docs.push_str("Accepted forms:\n\n");
        for variant in &item.variants {
            let variant_doc = required_doc(
                &variant.attrs,
                &format!("{element_name}::{}", variant.ident),
            )?;
            match &variant.fields {
                Fields::Unnamed(fields) => {
                    writeln!(docs, "- {variant_doc}")?;
                    for (index, nested) in fields.unnamed.iter().enumerate() {
                        let prose = required_doc(
                            &nested.attrs,
                            &format!("{element_name}::{} field {index}", variant.ident),
                        )?;
                        writeln!(docs, "  - {}", prose.replace('\n', " "))?;
                    }
                }
                Fields::Unit => {
                    writeln!(docs, "- {variant_doc}")?;
                }
                Fields::Named(fields) => {
                    writeln!(docs, "- {variant_doc}")?;
                    for nested in &fields.named {
                        let name = field_name(nested)?;
                        let prose = required_doc(
                            &nested.attrs,
                            &format!("{element_name}::{}::{name}", variant.ident),
                        )?;
                        writeln!(docs, "  - `{name}` — {}", prose.replace('\n', " "))?;
                    }
                }
            }
        }
        docs.push('\n');
    }
    Ok(())
}

fn render_keys(docs: &mut String) {
    docs.push_str("Assign a key to `\"noop\"` to unbind an inherited mapping.\n\n");
    docs.push_str(
        "Write chords with lowercase `ctrl+`, `alt+`, and `shift+` modifiers followed by a character or a named key such as `enter`, `esc`, `tab`, `backspace`, `delete`, an arrow, `home`, `end`, `pageup`, `pagedown`, or `space`.\n\n",
    );
    docs.push_str("Defaults:\n\n");
    docs.push_str(&default_keys_toml());
    docs.push('\n');
}

fn render_theme(
    docs: &mut String,
    theme: &ItemStruct,
    model: &ConfigModel,
    default: Option<&toml::Value>,
) -> Result<()> {
    let theme_color = model
        .enums
        .get("ThemeColor")
        .context("ThemeColor enum not found in the config module")?;
    let colors = theme_color
        .variants
        .iter()
        .map(|variant| format!("`{}`", snake_case(&variant.ident.to_string())))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        docs,
        "Colors use the terminal's ANSI palette and can be {colors}.\n"
    )?;
    docs.push_str("Defaults:\n\n```toml\n[theme]\n");
    for field in named_fields(theme)? {
        let name = field_name(field)?;
        let value = default
            .and_then(|value| value.get(&name))
            .with_context(|| format!("ThemeConfig::{name} is missing from Config::default()"))?;
        writeln!(docs, "{name} = {}", format_toml_value(value))?;
    }
    docs.push_str("```\n\n");
    Ok(())
}

fn render_default(docs: &mut String, default: Option<&toml::Value>) -> Result<()> {
    if let Some(value) = default.filter(|value| !is_empty_composite(value)) {
        writeln!(docs, "Default: `{}`\n", format_toml_value(value))?;
    }
    Ok(())
}

fn named_fields(
    item: &ItemStruct,
) -> Result<&syn::punctuated::Punctuated<Field, syn::token::Comma>> {
    match &item.fields {
        Fields::Named(fields) => Ok(&fields.named),
        _ => bail!("{} must use named fields", item.ident),
    }
}

fn field_name(field: &Field) -> Result<String> {
    field
        .ident
        .as_ref()
        .map(ToString::to_string)
        .context("expected a named field")
}

fn direct_type_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn collection_element_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(Type::Path(element)) = arguments.args.first()? else {
        return None;
    };
    element
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn extract_doc(attrs: &[Attribute]) -> String {
    attrs
        .iter()
        .filter_map(|attribute| {
            if !attribute.path().is_ident("doc") {
                return None;
            }
            let Meta::NameValue(meta) = &attribute.meta else {
                return None;
            };
            let syn::Expr::Lit(expression) = &meta.value else {
                return None;
            };
            let syn::Lit::Str(value) = &expression.lit else {
                return None;
            };
            Some(value.value().trim().to_owned())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn required_doc(attrs: &[Attribute], item: &str) -> Result<String> {
    let doc = extract_doc(attrs);
    if doc.is_empty() {
        bail!("missing user-facing doc comment on {item}");
    }
    Ok(doc)
}

fn push_doc(output: &mut String, doc: &str) {
    output.push_str(doc);
    output.push_str("\n\n");
}

fn push_distinct_doc(output: &mut String, first: &str, second: &str) {
    if first != second {
        push_doc(output, second);
    }
}

fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => format!("\"{}\"", value.replace('"', "\\\"")),
        other => other.to_string(),
    }
}

fn is_empty_composite(value: &toml::Value) -> bool {
    match value {
        toml::Value::Array(values) => values.is_empty(),
        toml::Value::Table(values) => values.is_empty(),
        _ => false,
    }
}

fn snake_case(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        output.push(character.to_ascii_lowercase());
    }
    output
}

fn replace_generated_section(readme: &str, docs: &str) -> Result<String> {
    let (before, remainder) = readme
        .split_once(CONFIG_START)
        .context("README is missing the config start marker")?;
    if before.contains(CONFIG_END) || remainder.matches(CONFIG_START).count() != 0 {
        bail!("README config markers are duplicated or out of order");
    }
    let (_, after) = remainder
        .split_once(CONFIG_END)
        .context("README is missing the config end marker")?;
    if after.contains(CONFIG_END) {
        bail!("README config markers are duplicated");
    }
    Ok(format!("{before}{CONFIG_START}\n{docs}{CONFIG_END}{after}"))
}

#[derive(Default)]
struct ConfigModel {
    structs: HashMap<String, ItemStruct>,
    enums: HashMap<String, ItemEnum>,
    visited: HashSet<PathBuf>,
}

impl ConfigModel {
    fn parse(path: &Path) -> Result<Self> {
        let mut model = Self::default();
        model.parse_file(path)?;
        Ok(model)
    }

    fn parse_file(&mut self, path: &Path) -> Result<()> {
        let canonical = fs::canonicalize(path)
            .with_context(|| format!("failed to resolve {}", path.display()))?;
        if !self.visited.insert(canonical) {
            return Ok(());
        }
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let syntax =
            parse_file(&source).with_context(|| format!("failed to parse {}", path.display()))?;
        for item in syntax.items {
            match item {
                Item::Struct(item) => {
                    self.structs.insert(item.ident.to_string(), item);
                }
                Item::Enum(item) => {
                    self.enums.insert(item.ident.to_string(), item);
                }
                Item::Mod(item) if item.content.is_none() => {
                    if let Some(module_path) = module_path(path, &item) {
                        self.parse_file(&module_path)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn module_path(source: &Path, module: &ItemMod) -> Option<PathBuf> {
    let parent = source.parent()?;
    let name = module.ident.to_string();
    [
        parent.join(format!("{name}.rs")),
        parent.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_the_marked_section() {
        let input = "before\n<!-- CONFIG:START -->\nstale\n<!-- CONFIG:END -->\nafter\n";
        assert_eq!(
            replace_generated_section(input, "fresh\n").unwrap(),
            "before\n<!-- CONFIG:START -->\nfresh\n<!-- CONFIG:END -->\nafter\n"
        );
    }

    #[test]
    fn formats_rust_names_as_serde_snake_case() {
        assert_eq!(snake_case("DarkGray"), "dark_gray");
    }

    #[test]
    fn normalizes_crlf_line_endings() {
        assert_eq!(
            normalize_line_endings("first\r\nsecond\r\n"),
            "first\nsecond\n"
        );
    }
}
