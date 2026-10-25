# Using malac with Lora Gateway

Lora Gateway is a LiteLLM-based aggregator that speaks both the OpenAI and the Anthropic wire formats, so malac can use it through either provider kind:

```bash
printf '%s' "$LORA_API_KEY" | malac provider add lora \
  --base-url https://lora.example.com/v1 --model default --api-key-stdin --set-default

printf '%s' "$LORA_API_KEY" | malac provider add lora-anthropic --kind anthropic \
  --base-url https://lora.example.com --model smart --api-key-stdin
```

Then `malac -m lora/smart ...` or `malac -m lora-anthropic/smart ...`.

## What was verified (2026-10-25)

- `/v1/models` lists `default`, `smart`, `fast`, `vision` and the concrete models behind them.
- Streaming chat completions with tool calls, `reasoning_content` deltas, usage with `cached_tokens` and `cost` in the final chunk.
- `/v1/messages` (Anthropic format) with tool use and thinking blocks. Thinking blocks come back with `signature: null`, so malac does not replay them.
- `default`, `fast` and `vision` accept images; `smart` returned no text for an image prompt within 200 output tokens.
- `/v1/embeddings` fails for every chat model (`This model does not support pooling`).

## Gateway changes that would help

None of these are required. Items 1 and 2 need no malac changes; the others would let malac drop per-model configuration.

1. **An embedding model alias**, e.g. `embed`, routed to a Together-hosted embedding model such as `BAAI/bge-large-en-v1.5` or `intfloat/multilingual-e5-large-instruct`. Vector memory needs `/v1/embeddings`; today it has to use a second provider.
2. **A dedicated malac key or route** (e.g. `/malac/v1`) so its traffic and spend can be tracked and rate-limited separately.
3. **Model metadata on `/v1/models`**: `max_output_tokens` and a `supports_vision` / `supports_reasoning` flag for every alias. malac could then size requests and route images without per-model config.
4. **Signed thinking passthrough** on `/v1/messages` when the upstream model is Anthropic, so extended thinking can be replayed across tool calls.
5. **Prompt caching passthrough**: honour `cache_control` on `/v1/messages`, and keep the cache-friendly prefix stable on the OpenAI route.
6. **Custom connections**: a way to register per-user upstream credentials (for example a personal Anthropic or OpenAI key) behind the same gateway key.
