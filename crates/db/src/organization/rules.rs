use crate::diesel_err;
use anyhow::Result;
use diesel::prelude::*;

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

// ============================================================================
// Diesel DSL CRUD
// ============================================================================

/// List all rules
pub fn list_rules(conn: &mut diesel::SqliteConnection) -> Result<Vec<DbOrganizationRule>> {
    use crate::diesel_schema::organization_rules::dsl::*;

    let results = organization_rules
        .order((priority.desc(), name.asc()))
        .load::<DbOrganizationRuleRow>(conn)
        .map_err(diesel_err("query"))?;

    Ok(results.into_iter().map(row_to_rule).collect())
}

/// Get a single rule by ID
pub fn get_rule(
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

    Ok(result.map(row_to_rule))
}

/// Delete a rule (system rules are immune to delete)
pub fn delete_rule(conn: &mut diesel::SqliteConnection, rule_id: i32) -> Result<()> {
    use crate::diesel_schema::organization_rules::dsl::*;

    diesel::delete(organization_rules.filter(id.eq(rule_id).and(is_system.eq(false))))
        .execute(conn)
        .map_err(diesel_err("delete"))?;

    Ok(())
}

/// Save a rule (Insert or Update)
pub fn save_rule(
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
                modified_at.eq(chrono::Utc::now().to_rfc3339()),
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
            .returning(id)
            .get_result(conn)
            .map_err(diesel_err("insert"))?;
        Ok(new_id as i64)
    }
}

// Helper to convert Diesel row to domain model
fn row_to_rule(r: DbOrganizationRuleRow) -> DbOrganizationRule {
    DbOrganizationRule {
        id: Some(r.id as i64),
        name: r.name,
        description: r.description,
        category: r.category,
        trigger_json: r.trigger_json,
        actions_json: r.actions_json,
        priority: r.priority,
        is_enabled: r.is_enabled,
        is_system: r.is_system,
    }
}
