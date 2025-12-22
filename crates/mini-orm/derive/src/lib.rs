//! Derive macro for database configuration structs.
//!
//! Provides `#[derive(DbConfig)]` which generates:
//! - CREATE TABLE SQL statement
//! - from_row() implementation
//! - save() and load() methods
//!
//! # Example
//! ```ignore
//! #[derive(DbConfig)]
//! #[db_table = "user_config"]
//! pub struct UserConfig {
//!     #[db(primary_key)]
//!     pub id: i32,
//!     
//!     #[db(default = "native")]
//!     pub backend_mode: String,
//!     
//!     #[db(skip)]
//!     pub cached_value: String,
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Lit, Meta, Type};

/// Derive macro for database config structs
#[proc_macro_derive(DbConfig, attributes(db_table, db))]
pub fn derive_db_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let table_name = get_table_name(&input);

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => panic!("DbConfig only supports structs with named fields"),
        },
        _ => panic!("DbConfig only supports structs"),
    };

    let mut column_defs = Vec::new();
    let mut from_row_fields = Vec::new();
    let mut insert_columns = Vec::new();
    let mut insert_placeholders = Vec::new();
    let mut insert_values = Vec::new();
    let mut alter_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut idx = 0usize;

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        // Parse field attributes
        let attrs = parse_field_attrs(&field.attrs);

        if attrs.skip {
            continue;
        }

        let col_name = field_name.to_string();
        let sql_type = rust_type_to_sql(field_type);

        // Build column definition
        let mut col_def = format!("{} {}", col_name, sql_type);

        if attrs.primary_key {
            col_def.push_str(" PRIMARY KEY");
            if sql_type == "INTEGER" {
                col_def.push_str(" CHECK (id = 1)"); // Single row pattern
            }
        }

        if let Some(default) = &attrs.default {
            col_def.push_str(&format!(" DEFAULT {}", quote_sql_value(default, &sql_type)));
        }

        if !attrs.nullable && !is_option_type(field_type) {
            col_def.push_str(" NOT NULL");
        }

        column_defs.push(col_def.clone());

        // Generate ALTER statement for migration (skip primary key)
        if !attrs.primary_key {
            let alter_query = format!("ALTER TABLE {} ADD COLUMN {}", table_name, col_def);
            alter_stmts.push(quote! {
                // Ignore "duplicate column name" error
                let _ = conn.execute(#alter_query, []);
            });
        }

        // Build from_row extraction
        let idx_lit = syn::LitInt::new(&idx.to_string(), proc_macro2::Span::call_site());

        if is_option_type(field_type) {
            from_row_fields.push(quote! {
                #field_name: row.get(#idx_lit).ok()
            });
        } else if is_pathbuf_type(field_type) {
            from_row_fields.push(quote! {
                #field_name: row.get::<_, Option<String>>(#idx_lit)
                    .ok()
                    .flatten()
                    .map(std::path::PathBuf::from)
            });
        } else {
            from_row_fields.push(quote! {
                #field_name: row.get(#idx_lit)?
            });
        }

        // Build insert statement parts (skip primary key for insert)
        if !attrs.primary_key {
            insert_columns.push(col_name.clone());
            insert_placeholders.push(format!("?{}", insert_columns.len()));

            let type_str = quote!(#field_type).to_string();

            if type_str.contains("PathBuf") {
                // PathBuf types need conversion
                if type_str.starts_with("Option") {
                    insert_values.push(quote! {
                        self.#field_name.as_ref().map(|p| p.to_string_lossy().to_string())
                    });
                } else {
                    insert_values.push(quote! {
                        self.#field_name.to_string_lossy().to_string()
                    });
                }
            } else if type_str.starts_with("Option") {
                // Option<String> or other Option types - just clone
                insert_values.push(quote! {
                    self.#field_name.clone()
                });
            } else {
                // Regular types - reference
                insert_values.push(quote! {
                    &self.#field_name
                });
            }
        }

        idx += 1;
    }

    // Generate CREATE TABLE SQL
    let create_sql = format!(
        "CREATE TABLE IF NOT EXISTS {} (\n    {}\n)",
        table_name,
        column_defs.join(",\n    ")
    );

    // Generate column select list for from_row
    let select_cols: Vec<String> = fields
        .iter()
        .filter_map(|f| {
            let attrs = parse_field_attrs(&f.attrs);
            if attrs.skip {
                None
            } else {
                Some(f.ident.as_ref().unwrap().to_string())
            }
        })
        .collect();
    let select_sql = format!(
        "SELECT {} FROM {} WHERE id = 1",
        select_cols.join(", "),
        table_name
    );

    // Generate INSERT/UPDATE SQL (upsert)
    let upsert_sql = format!(
        "INSERT INTO {} (id, {}) VALUES (1, {}) \
         ON CONFLICT(id) DO UPDATE SET {}",
        table_name,
        insert_columns.join(", "),
        insert_placeholders.join(", "),
        insert_columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{} = ?{}", c, i + 1))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let expanded = quote! {
        impl #name {
            /// SQL to create the table
            pub const CREATE_TABLE_SQL: &'static str = #create_sql;

            /// Load from database connection
            pub fn load(conn: &rusqlite::Connection) -> rusqlite::Result<Option<Self>> {
                let mut stmt = conn.prepare(#select_sql)?;
                let mut rows = stmt.query([])?;

                if let Some(row) = rows.next()? {
                    Ok(Some(Self {
                        #(#from_row_fields),*
                    }))
                } else {
                    Ok(None)
                }
            }

            /// Save to database connection (upsert)
            pub fn save(&self, conn: &rusqlite::Connection) -> rusqlite::Result<()> {
                conn.execute(
                    #upsert_sql,
                    rusqlite::params![#(#insert_values),*]
                )?;
                Ok(())
            }

            /// Ensure the table exists and has all columns
            pub fn ensure_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
                // 1. Create table if not exists with all columns
                conn.execute(Self::CREATE_TABLE_SQL, [])?;

                // 2. Attempt to add columns (migration) - ignoring duplicates
                #(#alter_stmts)*

                Ok(())
            }
        }
    };

    TokenStream::from(expanded)
}

#[derive(Default)]
struct FieldAttrs {
    primary_key: bool,
    default: Option<String>,
    nullable: bool,
    skip: bool,
}

fn parse_field_attrs(attrs: &[syn::Attribute]) -> FieldAttrs {
    let mut result = FieldAttrs::default();

    for attr in attrs {
        if attr.path().is_ident("db") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("primary_key") {
                    result.primary_key = true;
                } else if meta.path.is_ident("nullable") {
                    result.nullable = true;
                } else if meta.path.is_ident("skip") {
                    result.skip = true;
                } else if meta.path.is_ident("default") {
                    let value: Lit = meta.value()?.parse()?;
                    if let Lit::Str(s) = value {
                        result.default = Some(s.value());
                    }
                }
                Ok(())
            });
        }
    }

    result
}

fn get_table_name(input: &DeriveInput) -> String {
    for attr in &input.attrs {
        if attr.path().is_ident("db_table") {
            if let Meta::NameValue(nv) = &attr.meta {
                if let Expr::Lit(expr_lit) = &nv.value {
                    if let Lit::Str(s) = &expr_lit.lit {
                        return s.value();
                    }
                }
            }
        }
    }
    // Default: snake_case of struct name
    to_snake_case(&input.ident.to_string())
}

fn rust_type_to_sql(ty: &Type) -> &'static str {
    let type_str = quote!(#ty).to_string().replace(" ", "");

    if type_str.contains("i32") || type_str.contains("i64") || type_str.contains("bool") {
        "INTEGER"
    } else if type_str.contains("f32") || type_str.contains("f64") {
        "REAL"
    } else {
        "TEXT"
    }
}

fn is_option_type(ty: &Type) -> bool {
    let type_str = quote!(#ty).to_string();
    type_str.starts_with("Option")
}

fn is_pathbuf_type(ty: &Type) -> bool {
    let type_str = quote!(#ty).to_string();
    type_str.contains("PathBuf")
}

fn quote_sql_value(value: &str, sql_type: &str) -> String {
    if sql_type == "INTEGER" || sql_type == "REAL" {
        value.to_string()
    } else {
        format!("'{}'", value)
    }
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}
