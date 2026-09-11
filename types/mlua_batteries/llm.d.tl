-- mlua-batteries `llm`: chat completion across providers (openai,
-- anthropic, ollama, plus any the host registered). Both the HTTP and
-- the LLM policy must allow a request; `chat` raises on a denied or
-- failed request, `batch` reports per-entry errors instead.
local record llm
   record ContentPart
      -- "text" | "image_url" | "image_base64"
      type: string
      text: string
      url: string
      data: string
      media_type: string
   end

   record Message
      -- "user" | "assistant" (system prompts go in Request.system)
      role: string
      content: string | {ContentPart}
   end

   record Request
      provider: string
      model: string
      -- One of prompt / messages.
      prompt: string
      messages: {Message}
      system: string
      max_tokens: integer
      temperature: number
      stop: {string}
      api_key: string
      base_url: string
      -- Seconds.
      timeout: integer
      -- Provider-specific fields merged into the request body.
      extra: {string: any}
   end

   record Usage
      input_tokens: integer
      output_tokens: integer
   end

   record Response
      content: string
      -- "stop" | "max_tokens" | "content_filter" | "error"
      finish_reason: string
      usage: Usage
      model: string
      -- Only in `batch` results: set when that request failed, and the
      -- other fields are absent.
      error: string
   end

   chat: function(Request): Response
   -- Runs the requests in parallel; results are in request order.
   batch: function({Request}): {Response}
end

return llm
