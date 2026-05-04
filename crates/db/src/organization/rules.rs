use crate::diesel_err;
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

/// Diesel-compatible query result for organization rules
#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::diesel_schema::organization_rules)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct DbOrganizationRuleRow {
    pub id: i32,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub priority: i32,
    pub is_enabled: bool,
    pub is_system: bool,
    pub trigger_json: String,
    pub actions_json: String,
    pub created_at: String,
    pub modified_at: Option<String>,
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

// ============================================================================
// Diesel DSL versions
// ============================================================================

use diesel::prelude::*;

/// List all rules using Diesel DSL
pub fn list_rules_diesel(conn: &mut diesel::SqliteConnection) -> Result<Vec<DbOrganizationRule>> {
    use crate::diesel_schema::organization_rules::dsl::*;

    let results = organization_rules
        .order((priority.desc(), name.asc()))
        .load::<DbOrganizationRuleRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(results
        .into_iter()
        .map(|r| DbOrganizationRule {
            id: Some(r.id as i64),
            name: r.name,
            description: r.description,
            category: r.category,
            trigger_json: r.trigger_json,
            actions_json: r.actions_json,
            priority: r.priority,
            is_enabled: r.is_enabled,
            is_system: r.is_system,
        })
        .collect())
}

/// Get a single rule by ID using Diesel DSL
pub fn get_rule_diesel(
    conn: &mut diesel::SqliteConnection,
    rule_id: i32,
) -> Result<Option<DbOrganizationRule>> {
    use crate::diesel_schema::organization_rules::dsl::*;
    use diesel::result::OptionalExtension;

    let result = organization_rules
        .filter(id.eq(rule_id))
        .first::<DbOrganizationRuleRow>(conn)
        .optional()
        .map_err(diesel_err("query"))?;

    Ok(result.map(|r| DbOrganizationRule {
        id: Some(r.id as i64),
        name: r.name,
        description: r.description,
        category: r.category,
        trigger_json: r.trigger_json,
        actions_json: r.actions_json,
        priority: r.priority,
        is_enabled: r.is_enabled,
        is_system: r.is_system,
    }))
}

/// Delete a rule using Diesel DSL
pub fn delete_rule_diesel(conn: &mut diesel::SqliteConnection, rule_id: i32) -> Result<()> {
    use crate::diesel_schema::organization_rules::dsl::*;

    diesel::delete(organization_rules.filter(id.eq(rule_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Save a rule (Insert or Update) using Diesel DSL
pub fn save_rule_diesel(
    conn: &mut diesel::SqliteConnection,
    rule: &DbOrganizationRule,
) -> Result<i64> {
    use crate::diesel_schema::organization_rules::dsl::*;

    if let Some(rule_id) = rule.id {
        // Update
        diesel::update(organization_rules.filter(id.eq(rule_id as i32)))
            .set((
                name.eq(&rule.name),
                description.eq(&rule.description),
                category.eq(&rule.category),
                trigger_json.eq(&rule.trigger_json),
                actions_json.eq(&rule.actions_json),
                priority.eq(rule.priority),
                is_enabled.eq(rule.is_enabled),
                is_system.eq(rule.is_system),
                modified_at.eq(chrono::Utc::now().to_rfc3339()), // Use formatted string for Text column
            ))
            .execute(conn)
            .map_err(diesel_err("update"))?;
        Ok(rule_id)
    } else {
        // Insert
        let new_id: i32 = diesel::insert_into(organization_rules)
            .values((
                name.eq(&rule.name),
                description.eq(&rule.description),
                category.eq(&rule.category),
                trigger_json.eq(&rule.trigger_json),
                actions_json.eq(&rule.actions_json),
                priority.eq(rule.priority),
                is_enabled.eq(rule.is_enabled),
                is_system.eq(rule.is_system),
            ))
            // .returning(id) -- requires feature
            .returning(id)
            .get_result(conn)
            .map_err(diesel_err("insert"))?;
        Ok(new_id as i64)
    }
}
