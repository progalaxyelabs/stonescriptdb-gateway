use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::Serialize;

/// Represents a foreign key dependency between tables
#[derive(Debug, Clone, Serialize)]
pub struct ForeignKeyDependency {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

/// Represents a table with its dependencies
#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub primary_key: Option<Vec<String>>,
    pub foreign_keys: Vec<ForeignKeyDependency>,
    pub depends_on: Vec<String>,  // Tables this table depends on
}

/// Represents a column definition
#[derive(Debug, Clone, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub has_default: bool,
    pub references: Option<ColumnReference>,
}

/// Represents a column reference (inline foreign key)
#[derive(Debug, Clone, Serialize)]
pub struct ColumnReference {
    pub table: String,
    pub column: String,
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
}

/// Result of dependency analysis
#[derive(Debug, Clone, Serialize)]
pub struct DependencyAnalysis {
    pub tables: Vec<TableInfo>,
    pub creation_order: Vec<String>,
    pub dependency_graph: HashMap<String, Vec<String>>,
    pub reverse_dependencies: HashMap<String, Vec<String>>,
    pub circular_dependencies: Vec<Vec<String>>,
}

/// Analyzes table dependencies from SQL files
pub struct DependencyAnalyzer;

impl DependencyAnalyzer {
    /// Analyze all SQL files in a directory (migrations or tables folder)
    /// Supports .pssql and .pgsql extensions
    pub fn analyze_directory(dir: &Path) -> Result<DependencyAnalysis, String> {
        let mut all_sql = String::new();

        // Read all .pssql and .pgsql files
        let mut files: Vec<_> = fs::read_dir(dir)
            .map_err(|e| format!("Failed to read directory: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension()
                    .map(|ext| ext == "pssql" || ext == "pgsql" || ext == "sql")
                    .unwrap_or(false)
            })
            .collect();

        // Sort by filename (important for migrations)
        files.sort_by_key(|entry| entry.file_name());

        for entry in files {
            let content = fs::read_to_string(entry.path())
                .map_err(|e| format!("Failed to read {}: {}", entry.path().display(), e))?;
            all_sql.push_str(&content);
            all_sql.push('\n');
        }

        Self::analyze_sql(&all_sql)
    }

    /// Analyze SQL content for table dependencies
    pub fn analyze_sql(sql: &str) -> Result<DependencyAnalysis, String> {
        let tables = Self::extract_tables(sql);
        let dependency_graph = Self::build_dependency_graph(&tables);
        let reverse_dependencies = Self::build_reverse_dependencies(&dependency_graph);
        let circular_dependencies = Self::detect_circular_dependencies(&dependency_graph);
        let creation_order = Self::topological_sort(&dependency_graph)?;

        Ok(DependencyAnalysis {
            tables,
            creation_order,
            dependency_graph,
            reverse_dependencies,
            circular_dependencies,
        })
    }

    /// Extract table definitions from SQL
    fn extract_tables(sql: &str) -> Vec<TableInfo> {
        let mut tables = Vec::new();

        // Normalize SQL: remove comments and extra whitespace
        let sql = Self::normalize_sql(sql);

        // Find all CREATE TABLE statements
        let create_table_re = regex::Regex::new(
            r"(?is)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(\w+)\s*\((.*?)\)(?:\s*;|\s*$)"
        ).unwrap();

        for cap in create_table_re.captures_iter(&sql) {
            let table_name = cap[1].to_lowercase();
            let body = &cap[2];

            let (columns, foreign_keys, primary_key) = Self::parse_table_body(body, &table_name);

            // Extract tables this table depends on
            let depends_on: Vec<String> = foreign_keys
                .iter()
                .map(|fk| fk.to_table.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            tables.push(TableInfo {
                name: table_name,
                columns,
                primary_key,
                foreign_keys,
                depends_on,
            });
        }

        tables
    }

    /// Normalize SQL by removing comments
    fn normalize_sql(sql: &str) -> String {
        // Remove single-line comments
        let single_line_re = regex::Regex::new(r"--[^\n]*").unwrap();
        let sql = single_line_re.replace_all(sql, "");

        // Remove multi-line comments
        let multi_line_re = regex::Regex::new(r"/\*[\s\S]*?\*/").unwrap();
        let sql = multi_line_re.replace_all(&sql, "");

        sql.to_string()
    }

    /// Parse table body to extract columns and foreign keys
    fn parse_table_body(body: &str, _table_name: &str) -> (Vec<ColumnInfo>, Vec<ForeignKeyDependency>, Option<Vec<String>>) {
        let mut columns = Vec::new();
        let mut foreign_keys = Vec::new();
        let mut primary_key: Option<Vec<String>> = None;

        // Split by comma, but handle nested parentheses
        let parts = Self::split_table_body(body);

        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let part_upper = part.to_uppercase();

            // Check for table-level PRIMARY KEY constraint
            if part_upper.starts_with("PRIMARY KEY") {
                if let Some(pk_cols) = Self::extract_primary_key_columns(part) {
                    primary_key = Some(pk_cols);
                }
                continue;
            }

            // Check for table-level FOREIGN KEY constraint
            if part_upper.starts_with("FOREIGN KEY") || part_upper.contains("FOREIGN KEY") {
                if let Some(fk) = Self::parse_table_level_foreign_key(part, _table_name) {
                    foreign_keys.push(fk);
                }
                continue;
            }

            // Check for CHECK constraint at table level
            if part_upper.starts_with("CHECK") || part_upper.starts_with("CONSTRAINT") {
                continue;
            }

            // Check for UNIQUE constraint at table level
            if part_upper.starts_with("UNIQUE") {
                continue;
            }

            // Parse as column definition
            if let Some(col) = Self::parse_column(part) {
                // Check for inline PRIMARY KEY
                if col.is_primary_key && primary_key.is_none() {
                    primary_key = Some(vec![col.name.clone()]);
                }

                // Check for inline REFERENCES
                if let Some(ref refs) = col.references {
                    foreign_keys.push(ForeignKeyDependency {
                        from_table: _table_name.to_string(),
                        from_column: col.name.clone(),
                        to_table: refs.table.clone(),
                        to_column: refs.column.clone(),
                        on_delete: refs.on_delete.clone(),
                        on_update: refs.on_update.clone(),
                    });
                }

                columns.push(col);
            }
        }

        (columns, foreign_keys, primary_key)
    }

    /// Split table body by commas, handling nested parentheses
    fn split_table_body(body: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut paren_depth = 0;

        for ch in body.chars() {
            match ch {
                '(' => {
                    paren_depth += 1;
                    current.push(ch);
                }
                ')' => {
                    paren_depth -= 1;
                    current.push(ch);
                }
                ',' if paren_depth == 0 => {
                    parts.push(current.trim().to_string());
                    current = String::new();
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        if !current.trim().is_empty() {
            parts.push(current.trim().to_string());
        }

        parts
    }

    /// Extract column names from PRIMARY KEY (col1, col2) syntax
    fn extract_primary_key_columns(part: &str) -> Option<Vec<String>> {
        let re = regex::Regex::new(r"(?i)PRIMARY\s+KEY\s*\(\s*([^)]+)\s*\)").unwrap();
        re.captures(part).map(|cap| {
            cap[1]
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .collect()
        })
    }

    /// Parse table-level FOREIGN KEY constraint
    fn parse_table_level_foreign_key(part: &str, table_name: &str) -> Option<ForeignKeyDependency> {
        let re = regex::Regex::new(
            r"(?is)FOREIGN\s+KEY\s*\(\s*(\w+)\s*\)\s*REFERENCES\s+(\w+)\s*\(\s*(\w+)\s*\)(.*)"
        ).unwrap();

        re.captures(part).map(|cap| {
            let on_delete = Self::extract_on_action(&cap[4], "DELETE");
            let on_update = Self::extract_on_action(&cap[4], "UPDATE");

            ForeignKeyDependency {
                from_table: table_name.to_string(),
                from_column: cap[1].to_lowercase(),
                to_table: cap[2].to_lowercase(),
                to_column: cap[3].to_lowercase(),
                on_delete,
                on_update,
            }
        })
    }

    /// Parse a column definition
    fn parse_column(part: &str) -> Option<ColumnInfo> {
        // Column definition pattern: name type [constraints...]
        let re = regex::Regex::new(
            r"(?i)^(\w+)\s+(\w+(?:\s*\([^)]+\))?(?:\s*\[\s*\])?)"
        ).unwrap();

        let caps = re.captures(part)?;
        let name = caps[1].to_lowercase();
        let data_type = caps[2].to_uppercase();

        let part_upper = part.to_uppercase();

        // Check for NOT NULL
        let is_nullable = !part_upper.contains("NOT NULL");

        // Check for PRIMARY KEY
        let is_primary_key = part_upper.contains("PRIMARY KEY");

        // Check for DEFAULT
        let has_default = part_upper.contains("DEFAULT") || part_upper.contains("SERIAL");

        // Check for REFERENCES (inline foreign key)
        let references = Self::parse_inline_reference(part);

        Some(ColumnInfo {
            name,
            data_type,
            is_nullable,
            is_primary_key,
            has_default,
            references,
        })
    }

    /// Parse inline REFERENCES constraint
    fn parse_inline_reference(part: &str) -> Option<ColumnReference> {
        let re = regex::Regex::new(
            r"(?is)REFERENCES\s+(\w+)\s*\(\s*(\w+)\s*\)(.*)"
        ).unwrap();

        re.captures(part).map(|cap| {
            let suffix = &cap[3];
            let on_delete = Self::extract_on_action(suffix, "DELETE");
            let on_update = Self::extract_on_action(suffix, "UPDATE");

            ColumnReference {
                table: cap[1].to_lowercase(),
                column: cap[2].to_lowercase(),
                on_delete,
                on_update,
            }
        })
    }

    /// Extract ON DELETE/ON UPDATE action
    fn extract_on_action(text: &str, action_type: &str) -> Option<String> {
        let pattern = format!(r"(?i)ON\s+{}\s+(CASCADE|RESTRICT|SET\s+NULL|SET\s+DEFAULT|NO\s+ACTION)", action_type);
        let re = regex::Regex::new(&pattern).unwrap();
        re.captures(text).map(|cap| cap[1].to_uppercase())
    }

    /// Build dependency graph: table -> tables it depends on
    fn build_dependency_graph(tables: &[TableInfo]) -> HashMap<String, Vec<String>> {
        tables
            .iter()
            .map(|t| (t.name.clone(), t.depends_on.clone()))
            .collect()
    }

    /// Build reverse dependency graph: table -> tables that depend on it
    fn build_reverse_dependencies(graph: &HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize all tables
        for table in graph.keys() {
            reverse.entry(table.clone()).or_default();
        }

        // Build reverse edges
        for (table, deps) in graph {
            for dep in deps {
                reverse.entry(dep.clone()).or_default().push(table.clone());
            }
        }

        reverse
    }

    /// Detect circular dependencies using DFS
    fn detect_circular_dependencies(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited = HashSet::new();
        let mut rec_stack = Vec::new();

        for table in graph.keys() {
            if !visited.contains(table) {
                Self::dfs_find_cycles(table, graph, &mut visited, &mut rec_stack, &mut cycles);
            }
        }

        cycles
    }

    /// DFS helper to find cycles
    fn dfs_find_cycles(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        rec_stack: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(node.to_string());
        rec_stack.push(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    Self::dfs_find_cycles(neighbor, graph, visited, rec_stack, cycles);
                } else if rec_stack.contains(neighbor) {
                    // Found a cycle
                    let cycle_start = rec_stack.iter().position(|x| x == neighbor).unwrap();
                    let cycle: Vec<String> = rec_stack[cycle_start..].to_vec();
                    cycles.push(cycle);
                }
            }
        }

        rec_stack.pop();
    }

    /// Topological sort to get creation order
    fn topological_sort(graph: &HashMap<String, Vec<String>>) -> Result<Vec<String>, String> {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut all_nodes: HashSet<String> = HashSet::new();

        // Collect all nodes and compute in-degrees
        for (node, deps) in graph {
            all_nodes.insert(node.clone());
            in_degree.entry(node.clone()).or_insert(0);
            for dep in deps {
                all_nodes.insert(dep.clone());
                *in_degree.entry(node.clone()).or_insert(0) += 1;
            }
        }

        // Ensure all referenced tables have an entry
        for node in &all_nodes {
            in_degree.entry(node.clone()).or_insert(0);
        }

        // Build reverse adjacency (who depends on whom)
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for (node, deps) in graph {
            for dep in deps {
                adj.entry(dep.clone()).or_default().push(node.clone());
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(node, _)| node.clone())
            .collect();
        queue.sort(); // Deterministic order

        let mut result = Vec::new();

        while let Some(node) = queue.pop() {
            result.push(node.clone());

            if let Some(dependents) = adj.get(&node) {
                for dependent in dependents {
                    if let Some(deg) = in_degree.get_mut(dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(dependent.clone());
                            queue.sort();
                        }
                    }
                }
            }
        }

        if result.len() != all_nodes.len() {
            return Err("Circular dependency detected - cannot determine creation order".to_string());
        }

        Ok(result)
    }

    /// Format dependency analysis as a readable string
    pub fn format_analysis(analysis: &DependencyAnalysis) -> String {
        let mut output = String::new();

        output.push_str("═══════════════════════════════════════════════════════════════\n");
        output.push_str("                    TABLE DEPENDENCY ANALYSIS\n");
        output.push_str("═══════════════════════════════════════════════════════════════\n\n");

        // Tables summary
        output.push_str(&format!("Found {} tables\n\n", analysis.tables.len()));

        // Creation order
        output.push_str("CREATION ORDER (tables must be created in this sequence):\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        for (i, table) in analysis.creation_order.iter().enumerate() {
            output.push_str(&format!("  {}. {}\n", i + 1, table));
        }
        output.push('\n');

        // Dependency graph
        output.push_str("DEPENDENCY GRAPH (table → depends on):\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        let mut sorted_tables: Vec<_> = analysis.dependency_graph.iter().collect();
        sorted_tables.sort_by_key(|(name, _)| *name);

        for (table, deps) in sorted_tables {
            if deps.is_empty() {
                output.push_str(&format!("  {} → (no dependencies)\n", table));
            } else {
                output.push_str(&format!("  {} → {}\n", table, deps.join(", ")));
            }
        }
        output.push('\n');

        // Reverse dependencies
        output.push_str("REVERSE DEPENDENCIES (table ← depended on by):\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        let mut sorted_reverse: Vec<_> = analysis.reverse_dependencies.iter().collect();
        sorted_reverse.sort_by_key(|(name, _)| *name);

        for (table, dependents) in sorted_reverse {
            if dependents.is_empty() {
                output.push_str(&format!("  {} ← (nothing depends on this)\n", table));
            } else {
                output.push_str(&format!("  {} ← {}\n", table, dependents.join(", ")));
            }
        }
        output.push('\n');

        // Foreign key details
        output.push_str("FOREIGN KEY DETAILS:\n");
        output.push_str("───────────────────────────────────────────────────────────────\n");
        for table in &analysis.tables {
            if !table.foreign_keys.is_empty() {
                output.push_str(&format!("  {}:\n", table.name));
                for fk in &table.foreign_keys {
                    let mut constraints = Vec::new();
                    if let Some(ref on_del) = fk.on_delete {
                        constraints.push(format!("ON DELETE {}", on_del));
                    }
                    if let Some(ref on_upd) = fk.on_update {
                        constraints.push(format!("ON UPDATE {}", on_upd));
                    }
                    let constraint_str = if constraints.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", constraints.join(", "))
                    };
                    output.push_str(&format!(
                        "    • {}.{} → {}.{}{}\n",
                        fk.from_table, fk.from_column, fk.to_table, fk.to_column, constraint_str
                    ));
                }
            }
        }
        output.push('\n');

        // Circular dependencies warning
        if !analysis.circular_dependencies.is_empty() {
            output.push_str("⚠️  CIRCULAR DEPENDENCIES DETECTED:\n");
            output.push_str("───────────────────────────────────────────────────────────────\n");
            for cycle in &analysis.circular_dependencies {
                output.push_str(&format!("  {} → {}\n", cycle.join(" → "), cycle[0]));
            }
            output.push('\n');
        }

        output.push_str("═══════════════════════════════════════════════════════════════\n");

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_table() {
        let sql = r#"
            CREATE TABLE users (
                user_id SERIAL PRIMARY KEY,
                email VARCHAR(255) NOT NULL UNIQUE
            );
        "#;

        let analysis = DependencyAnalyzer::analyze_sql(sql).unwrap();
        assert_eq!(analysis.tables.len(), 1);
        assert_eq!(analysis.tables[0].name, "users");
        assert_eq!(analysis.tables[0].columns.len(), 2);
    }

    #[test]
    fn test_parse_foreign_key() {
        let sql = r#"
            CREATE TABLE users (
                user_id SERIAL PRIMARY KEY
            );

            CREATE TABLE todos (
                todo_id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE
            );
        "#;

        let analysis = DependencyAnalyzer::analyze_sql(sql).unwrap();
        assert_eq!(analysis.tables.len(), 2);

        let todos = analysis.tables.iter().find(|t| t.name == "todos").unwrap();
        assert_eq!(todos.foreign_keys.len(), 1);
        assert_eq!(todos.foreign_keys[0].to_table, "users");
        assert_eq!(todos.foreign_keys[0].on_delete, Some("CASCADE".to_string()));
    }

    #[test]
    fn test_creation_order() {
        let sql = r#"
            CREATE TABLE users (user_id SERIAL PRIMARY KEY);
            CREATE TABLE todos (
                todo_id SERIAL PRIMARY KEY,
                user_id INTEGER REFERENCES users(user_id)
            );
            CREATE TABLE todo_tags (
                todo_id INTEGER REFERENCES todos(todo_id),
                tag_id INTEGER REFERENCES tags(tag_id)
            );
            CREATE TABLE tags (tag_id SERIAL PRIMARY KEY);
        "#;

        let analysis = DependencyAnalyzer::analyze_sql(sql).unwrap();

        // users and tags should come before todos
        // todos should come before todo_tags
        let user_pos = analysis.creation_order.iter().position(|x| x == "users").unwrap();
        let tags_pos = analysis.creation_order.iter().position(|x| x == "tags").unwrap();
        let todos_pos = analysis.creation_order.iter().position(|x| x == "todos").unwrap();
        let todo_tags_pos = analysis.creation_order.iter().position(|x| x == "todo_tags").unwrap();

        assert!(user_pos < todos_pos);
        assert!(tags_pos < todo_tags_pos);
        assert!(todos_pos < todo_tags_pos);
    }
}