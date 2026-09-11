-- mlua-batteries `hash`: SHA-256 as a lower-case hex string.
local record hash
   sha256: function(string): string
   -- Reads the file under the host's path policy.
   sha256_file: function(string): string
end

return hash
