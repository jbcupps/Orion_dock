"""
Pro Router Sidecar — LangChain-powered best-of-two provider comparison.

Exposes a FastAPI HTTP endpoint consumed by Orion API when Pro thinking
tier is selected and >= 2 healthy providers are connected.

Usage:
    pip install -r requirements.txt
    uvicorn main:app --host 0.0.0.0 --port 8100

Environment:
    PRO_ROUTER_PORT (default 8100)
"""

from __future__ import annotations

import asyncio
import os
import time
from typing import Any

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

app = FastAPI(title="Orion Pro Router", version="0.1.0")


# ── Request / Response models ────────────────────────────────────────


class ProviderConfig(BaseModel):
    name: str  # e.g. "anthropic", "openai"
    api_key: str
    model: str  # e.g. "claude-opus-4-20250514"


class MessageItem(BaseModel):
    role: str
    content: str


class CompareRequest(BaseModel):
    messages: list[MessageItem]
    provider_configs: list[ProviderConfig]  # exactly 2


class ProviderResult(BaseModel):
    provider: str
    model: str
    content: str
    latency_ms: int
    error: str | None = None


class CompareResponse(BaseModel):
    selected_provider: str
    selected_model: str
    content: str
    results: list[ProviderResult]
    reason: str


# ── Provider invocation helpers ──────────────────────────────────────


async def _invoke_provider(cfg: ProviderConfig, messages: list[MessageItem]) -> ProviderResult:
    """Call a single provider via LangChain and return result with timing."""
    start = time.monotonic()
    try:
        llm = _build_llm(cfg)
        lc_messages = _to_langchain_messages(messages)
        response = await llm.ainvoke(lc_messages)
        elapsed = int((time.monotonic() - start) * 1000)
        content = response.content if hasattr(response, "content") else str(response)
        return ProviderResult(
            provider=cfg.name,
            model=cfg.model,
            content=content,
            latency_ms=elapsed,
        )
    except Exception as e:
        elapsed = int((time.monotonic() - start) * 1000)
        return ProviderResult(
            provider=cfg.name,
            model=cfg.model,
            content="",
            latency_ms=elapsed,
            error=str(e),
        )


def _build_llm(cfg: ProviderConfig) -> Any:
    """Construct a LangChain ChatModel from provider config."""
    name = cfg.name.lower()
    if name == "anthropic":
        from langchain_anthropic import ChatAnthropic

        return ChatAnthropic(model=cfg.model, api_key=cfg.api_key, max_tokens=4096)
    elif name in ("openai", ""):
        from langchain_openai import ChatOpenAI

        return ChatOpenAI(model=cfg.model, api_key=cfg.api_key)
    elif name == "google":
        # Use OpenAI-compatible endpoint for Gemini
        from langchain_openai import ChatOpenAI

        return ChatOpenAI(
            model=cfg.model,
            api_key=cfg.api_key,
            base_url="https://generativelanguage.googleapis.com/v1beta/openai",
        )
    elif name == "xai":
        from langchain_openai import ChatOpenAI

        return ChatOpenAI(model=cfg.model, api_key=cfg.api_key, base_url="https://api.x.ai/v1")
    elif name == "perplexity":
        from langchain_openai import ChatOpenAI

        return ChatOpenAI(
            model=cfg.model, api_key=cfg.api_key, base_url="https://api.perplexity.ai"
        )
    else:
        from langchain_openai import ChatOpenAI

        return ChatOpenAI(model=cfg.model, api_key=cfg.api_key)


def _to_langchain_messages(messages: list[MessageItem]) -> list:
    """Convert our MessageItem list to LangChain message objects."""
    from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

    lc_msgs = []
    for m in messages:
        if m.role == "system":
            lc_msgs.append(SystemMessage(content=m.content))
        elif m.role == "assistant":
            lc_msgs.append(AIMessage(content=m.content))
        else:
            lc_msgs.append(HumanMessage(content=m.content))
    return lc_msgs


# ── Selection logic ──────────────────────────────────────────────────


def _select_best(results: list[ProviderResult]) -> tuple[ProviderResult, str]:
    """
    Choose the best response from two provider results.

    Strategy:
    1. Filter out errors — if one failed, pick the other.
    2. Filter out empty responses.
    3. Prefer the response with more substance (longer non-trivial content).
    4. Tie-break by latency (faster wins).
    """
    valid = [r for r in results if not r.error and r.content.strip()]
    if not valid:
        # Both failed; return first with error info
        return results[0], "both_failed"
    if len(valid) == 1:
        other = [r for r in results if r not in valid]
        reason = f"only_{valid[0].provider}_succeeded"
        if other and other[0].error:
            reason += f" ({other[0].provider} error: {other[0].error[:80]})"
        return valid[0], reason

    # Both succeeded — compare content quality heuristically
    a, b = valid[0], valid[1]
    len_a, len_b = len(a.content.strip()), len(b.content.strip())

    # If one response is substantially longer (>30% more), prefer it
    if len_a > len_b * 1.3:
        return a, f"longer_response ({a.provider}: {len_a} vs {b.provider}: {len_b} chars)"
    if len_b > len_a * 1.3:
        return b, f"longer_response ({b.provider}: {len_b} vs {a.provider}: {len_a} chars)"

    # Similar length — prefer faster response
    if a.latency_ms <= b.latency_ms:
        return a, f"faster ({a.provider}: {a.latency_ms}ms vs {b.provider}: {b.latency_ms}ms)"
    return b, f"faster ({b.provider}: {b.latency_ms}ms vs {a.provider}: {a.latency_ms}ms)"


# ── API endpoint ─────────────────────────────────────────────────────


@app.post("/compare", response_model=CompareResponse)
async def compare_providers(req: CompareRequest) -> CompareResponse:
    """
    Call two providers in parallel via LangChain RunnableParallel pattern,
    then select the best response.
    """
    if len(req.provider_configs) < 2:
        raise HTTPException(status_code=400, detail="Exactly 2 provider_configs required")

    configs = req.provider_configs[:2]

    # Run both providers concurrently (LangChain RunnableParallel concept)
    results = await asyncio.gather(
        _invoke_provider(configs[0], req.messages),
        _invoke_provider(configs[1], req.messages),
    )

    best, reason = _select_best(list(results))

    return CompareResponse(
        selected_provider=best.provider,
        selected_model=best.model,
        content=best.content,
        results=list(results),
        reason=reason,
    )


@app.get("/health")
async def health():
    return {"status": "ok"}


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PRO_ROUTER_PORT", "8100"))
    uvicorn.run("main:app", host="0.0.0.0", port=port)
