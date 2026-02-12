use std::borrow::Cow;

use crate::config::{
    BuiltinHook, BuiltinRepo, FilePattern, LocalRepo, MetaHook, MetaRepo, RemoteHook, RemoteRepo,
    Repo,
};

impl schemars::JsonSchema for FilePattern {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("FilePattern")
    }

    fn json_schema(_gen: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A file pattern, either a regex or glob pattern(s).",
            "oneOf": [
                {
                    "type": "string",
                    "description": "A regular expression pattern.",
                },
                {
                    "type": "object",
                    "properties": {
                        "glob": {
                            "oneOf": [
                                {
                                    "type": "string",
                                    "description": "A glob pattern.",
                                },
                                {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                    },
                                    "description": "A list of glob patterns.",
                                }
                            ]
                        }
                    },
                    "required": ["glob"],
                }
            ],
        })
    }
}

fn predefined_hook_schema(
    schema_gen: &mut schemars::SchemaGenerator,
    description: &str,
    id_schema: schemars::Schema,
) -> schemars::Schema {
    let mut schema = <RemoteHook as schemars::JsonSchema>::json_schema(schema_gen);

    let root = schema.ensure_object();
    root.insert("description".to_string(), serde_json::json!(description));
    root.insert("required".to_string(), serde_json::json!(["id"]));

    let properties = root
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut);

    if let Some(properties) = properties {
        properties.insert("id".to_string(), id_schema.into());
        properties.insert(
            "language".to_string(),
            serde_json::json!({
                "anyOf": [
                    {
                        "type": "string",
                        "enum": ["system"],
                        "description": "Language must be `system` for predefined hooks (or omitted).",
                    },
                    { "type": "null" }
                ]
            })
        );
        // `entry` is not allowed for predefined hooks.
        properties.insert(
            "entry".to_string(),
            serde_json::json!({
                "const": false,
                "description": "Entry is not allowed for predefined hooks.",
            }),
        );
    }

    schema
}

impl schemars::JsonSchema for MetaHook {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("MetaHook")
    }

    fn json_schema(schema_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use crate::hooks::MetaHooks;

        let id_schema = schema_gen.subschema_for::<MetaHooks>();
        predefined_hook_schema(schema_gen, "A meta hook predefined in prek.", id_schema)
    }
}

impl schemars::JsonSchema for BuiltinHook {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("BuiltinHook")
    }

    fn json_schema(r#gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use crate::hooks::BuiltinHooks;

        let id_schema = r#gen.subschema_for::<BuiltinHooks>();
        predefined_hook_schema(r#gen, "A builtin hook predefined in prek.", id_schema)
    }
}

pub(crate) fn schema_repo_local(
    _gen: &mut schemars::generate::SchemaGenerator,
) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "const": "local",
        "description": "Must be `local`.",
    })
}

pub(crate) fn schema_repo_meta(_gen: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "const": "meta",
        "description": "Must be `meta`.",
    })
}

pub(crate) fn schema_repo_builtin(
    _gen: &mut schemars::generate::SchemaGenerator,
) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "const": "builtin",
        "description": "Must be `builtin`.",
    })
}

pub(crate) fn schema_repo_remote(
    _gen: &mut schemars::generate::SchemaGenerator,
) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "not": {
            "enum": ["local", "meta", "builtin"],
        },
        "description": "Remote repository location. Must not be `local`, `meta`, or `builtin`.",
    })
}

impl schemars::JsonSchema for Repo {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Repo")
    }

    fn json_schema(r#gen: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
        let remote_schema = r#gen.subschema_for::<RemoteRepo>();
        let local_schema = r#gen.subschema_for::<LocalRepo>();
        let meta_schema = r#gen.subschema_for::<MetaRepo>();
        let builtin_schema = r#gen.subschema_for::<BuiltinRepo>();

        schemars::json_schema!({
            "type": "object",
            "description": "A repository of hooks, which can be remote, local, meta, or builtin.",
            "oneOf": [
                remote_schema,
                local_schema,
                meta_schema,
                builtin_schema,
            ],
        })
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "schemars"))]
mod _gen {
    use crate::config::Config;
    use anyhow::bail;
    use prek_consts::env_vars::EnvVars;
    use pretty_assertions::StrComparison;
    use std::path::PathBuf;

    const ROOT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");

    enum Mode {
        /// Update the content.
        Write,

        /// Don't write to the file, check if the file is up-to-date and error if not.
        Check,

        /// Write the generated help to stdout.
        DryRun,
    }

    fn generate() -> String {
        let settings = schemars::generate::SchemaSettings::draft07();
        let generator = schemars::SchemaGenerator::new(settings);
        let mut schema = generator.into_root_schema_for::<Config>();
        schema.insert(
            "$id".to_string(),
            "https://www.schemastore.org/prek.json".into(),
        );
        schema.insert("x-tombi-toml-version".to_string(), "v1.1.0".into());

        serde_json::to_string_pretty(&schema).unwrap() + "\n"
    }

    #[test]
    fn generate_json_schema() -> anyhow::Result<()> {
        let mode = if EnvVars::is_set(EnvVars::PREK_GENERATE) {
            Mode::Write
        } else {
            Mode::Check
        };

        let schema_string = generate();
        let filename = "prek.schema.json";
        let schema_path = PathBuf::from(ROOT_DIR).join(filename);

        match mode {
            Mode::DryRun => {
                anstream::println!("{schema_string}");
            }
            Mode::Check => match fs_err::read_to_string(schema_path) {
                Ok(current) => {
                    if current == schema_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        let comparison = StrComparison::new(&current, &schema_string);
                        bail!(
                            "{filename} changed, please run `mise run generate` to update:\n{comparison}"
                        );
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    bail!("{filename} not found, please run `mise run generate` to generate");
                }
                Err(err) => {
                    bail!("{filename} changed, please run `mise run generate` to update:\n{err}");
                }
            },
            Mode::Write => match fs_err::read_to_string(&schema_path) {
                Ok(current) => {
                    if current == schema_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        anstream::println!("Updating: {filename}");
                        fs_err::write(schema_path, schema_string.as_bytes())?;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    anstream::println!("Updating: {filename}");
                    fs_err::write(schema_path, schema_string.as_bytes())?;
                }
                Err(err) => {
                    bail!("{filename} changed, please run `mise run generate` to update:\n{err}");
                }
            },
        }

        Ok(())
    }
}
