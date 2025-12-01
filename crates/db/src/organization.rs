use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub struct DbOrganizationRule {
    pub id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub trigger_json: String,
    pub actions_json: String,
    pub priority: i32,
    pub is_enabled: bool,
    pub is_system: bool,
}

pub fn list_rules(conn: &Connection) -> Result<Vec<DbOrganizationRule>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, category, trigger_json, actions_json, priority, is_enabled, is_system 
         FROM organization_rules 
         ORDER BY priority DESC, name ASC"
    )?;

    let rules = stmt
        .query_map([], |row| {
            Ok(DbOrganizationRule {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                category: row.get(3)?,
                trigger_json: row.get(4)?,
                actions_json: row.get(5)?,
                priority: row.get(6)?,
                is_enabled: row.get(7)?,
                is_system: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rules)
}

pub fn get_rule(conn: &Connection, id: i64) -> Result<Option<DbOrganizationRule>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, category, trigger_json, actions_json, priority, is_enabled, is_system 
         FROM organization_rules 
         WHERE id = ?1"
    )?;

    let rule = stmt
        .query_row([id], |row| {
            Ok(DbOrganizationRule {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                category: row.get(3)?,
                trigger_json: row.get(4)?,
                actions_json: row.get(5)?,
                priority: row.get(6)?,
                is_enabled: row.get(7)?,
                is_system: row.get(8)?,
            })
        })
        .optional()?;

    Ok(rule)
}

pub fn save_rule(conn: &Connection, rule: &DbOrganizationRule) -> Result<i64> {
    if let Some(id) = rule.id {
        // Update
        conn.execute(
            "UPDATE organization_rules 
             SET name = ?1, description = ?2, category = ?3, trigger_json = ?4, actions_json = ?5, 
                 priority = ?6, is_enabled = ?7, is_system = ?8
             WHERE id = ?9",
            params![
                rule.name,
                rule.description,
                rule.category,
                rule.trigger_json,
                rule.actions_json,
                rule.priority,
                rule.is_enabled,
                rule.is_system,
                id
            ],
        )
        .context("Failed to update rule")?;
        Ok(id)
    } else {
        // Insert
        conn.execute(
            "INSERT INTO organization_rules 
             (name, description, category, trigger_json, actions_json, priority, is_enabled, is_system)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rule.name,
                rule.description,
                rule.category,
                rule.trigger_json,
                rule.actions_json,
                rule.priority,
                rule.is_enabled,
                rule.is_system
            ],
        ).context("Failed to insert rule")?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn delete_rule(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM organization_rules WHERE id = ?1 AND is_system = 0",
        [id],
    )?;
    Ok(())
}
