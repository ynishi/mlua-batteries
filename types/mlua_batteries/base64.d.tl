-- mlua-batteries `base64`: standard (padded) and URL-safe (unpadded)
-- alphabets. Decoding raises on malformed input or non-UTF-8 output.
local record base64
   encode: function(string): string
   decode: function(string): string
   encode_url: function(string): string
   decode_url: function(string): string
end

return base64
