-- mlua-batteries `validate`: check a table against a schema table.
--
-- A field's schema is either a type name ("string", "number", "integer",
-- "boolean", "table") or a Rule.
local record validate
   record Rule
      type: string
      required: boolean
      min: number
      max: number
      min_len: integer
      max_len: integer
      one_of: {any}
   end

   -- true on success; false plus the list of messages otherwise.
   check: function(table, {string: any}): boolean, {string}
end

return validate
