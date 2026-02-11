//! SQLite-backed memory store implementation.

use crate::schema::{CREATE_BIRTH, CREATE_MEMORIES};
use crate::store::{Memory, MemoryWeight, Result, StoreError};
use chrono::Utc;
use orion_core::AppConfig;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_conn(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_conn(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init_conn(conn: &Connection) -> Result<()> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(CREATE_MEMORIES)?;
        conn.execute_batch(CREATE_BIRTH)?;
        Ok(())
    }

    pub fn open_with_config(config: &AppConfig) -> Result<Self> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        Self::open(&config.db_path)
    }

    pub fn has_birth(&self) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM birth WHERE id = 1", [], |row| {
            row.get(0)
        })?;
        Ok(count > 0)
    }

    pub fn record_birth(&self, memory: &Memory) -> Result<()> {
        if self.has_birth()? {
            return Err(StoreError::BirthAlreadyRecorded);
        }
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        conn.execute(
            "INSERT INTO birth (id, content, created_at) VALUES (1, ?1, ?2)",
            [
                memory.content.as_str(),
                memory.created_at.to_rfc3339().as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn insert_memory(&self, memory: &Memory) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        conn.execute(
            "INSERT INTO memories (id, content, weight, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                memory.id,
                memory.content,
                memory.weight.as_str(),
                memory.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn count_memories(&self) -> Result<u64> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    pub fn vacuum(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        conn.execute("VACUUM", [])?;
        Ok(())
    }

    pub fn clear_memories(&self) -> Result<u64> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        let deleted = conn.execute("DELETE FROM memories", [])?;
        Ok(deleted as u64)
    }

    pub fn recent_memories(&self, limit: usize) -> Result<Vec<Memory>> {
        let conn = self.conn.lock().map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(e.to_string())))
        })?;
        let mut stmt = conn.prepare(
            "SELECT id, content, weight, created_at FROM memories ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            let created_at: String = row.get(3)?;
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            let weight_str: String = row.get(2)?;
            let weight = match weight_str.as_str() {
                "ephemeral" => MemoryWeight::Ephemeral,
                "distilled" => MemoryWeight::Distilled,
                "crystallized" => MemoryWeight::Crystallized,
                w => {
                    return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                        StoreError::InvalidData(format!("unknown weight: {}", w)),
                    )));
                }
            };
            Ok(Memory {
                id: row.get(0)?,
                content: row.get(1)?,
                weight,
                created_at,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
