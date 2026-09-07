---
name: summarize
description: Summarize web pages, local files, or long text using the built-in web_fetch/read_file tools plus your own reasoning. Use when the user asks to "summarize this URL/article/page/file" or wants a TL;DR of anything.
---

# Summarize

Summarize URLs and files with tools you already have — no external CLI, no API keys.

## Workflow

1. Fetch the raw content:
   - **URL** → `web_fetch` tool (returns the page as markdown).
   - **Local file** → `read_file` tool; for very large files, read in chunks and
     summarize progressively instead of trying to hold it all at once.
2. Summarize the content yourself at the depth the user asked for.
3. Point to section headings or line ranges so the user can drill into the source.

## Depth control

Match what the user asked for:

- "TL;DR" → 1-3 sentences.
- "summarize" → short paragraph + key bullet points.
- "detailed summary" → structured outline that follows the source's own headings.

## Honest boundaries

- If a URL is blocked or fails to fetch, say so plainly; do not invent content
  from the title alone.
- Video/audio links: you can only see the page text (title, description, and a
  transcript if the page includes one). Say when a real transcript is unavailable.
- For huge files, state how much of the file your summary actually covers.
