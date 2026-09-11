-- mlua-batteries `uuid`: random (v4) and time-ordered (v7) UUIDs as
-- lower-case hyphenated strings.
local record uuid
   v4: function(): string
   v7: function(): string
end

return uuid
