//! UUID generation module.
//!
//! ```lua
//! local uuid = std.uuid
//! local id4 = uuid.v4()   --> "550e8400-e29b-41d4-a716-446655440000"
//! local id7 = uuid.v7()   --> "018e4f6c-..."
//! ```

use mlua::prelude::*;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "v4",
        lua.create_function(|_, ()| Ok(::uuid::Uuid::new_v4().to_string()))?,
    )?;

    t.set(
        "v7",
        lua.create_function(|_, ()| Ok(::uuid::Uuid::now_v7().to_string()))?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    #[test]
    fn v4_format() {
        let s: String = eval(r#"return std.uuid.v4()"#);
        assert_eq!(s.len(), 36);
        assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
        // Version nibble is '4'
        assert_eq!(s.as_bytes()[14], b'4');
    }

    #[test]
    fn v4_unique() {
        let s: String = eval(
            r#"
            local a = std.uuid.v4()
            local b = std.uuid.v4()
            return a .. "|" .. b
        "#,
        );
        let parts: Vec<&str> = s.split('|').collect();
        assert_ne!(parts[0], parts[1]);
    }

    #[test]
    fn v7_format() {
        let s: String = eval(r#"return std.uuid.v7()"#);
        assert_eq!(s.len(), 36);
        // Version nibble is '7'
        assert_eq!(s.as_bytes()[14], b'7');
    }

    #[test]
    fn v7_monotonic() {
        let s: String = eval(
            r#"
            local a = std.uuid.v7()
            local b = std.uuid.v7()
            return a .. "|" .. b
        "#,
        );
        let parts: Vec<&str> = s.split('|').collect();
        assert!(parts[0] <= parts[1], "v7 should be monotonic");
    }
}
