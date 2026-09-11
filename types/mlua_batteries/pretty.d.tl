-- mlua-batteries `pretty`: deterministic dump of any Lua value in table
-- constructor syntax, for debugging, snapshots and test diffs. Never
-- raises: functions, userdata, cycles and depth overflow are rendered as
-- `<function>`, `<userdata>`, `<cycle>` and `{...}`.
local record pretty
   record Options
      -- Spaces per level; 0 prints on one line. Default 2.
      indent: integer
      -- Tables nested deeper than this print as `{...}`. Default unlimited.
      depth: integer
      -- Default true: array part first, then keys in a fixed order.
      sort_keys: boolean
   end

   dump: function(any, Options): string
end

return pretty
