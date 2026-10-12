// Minimal MCP client for a hydra-browser (Streamable HTTP), used by the e2e
// test and for demos. Each invocation is one MCP session.
//
//   node mcp.mjs <mcp-url> tools
//   node mcp.mjs <mcp-url> call <tool> ['<json args>']
//   node mcp.mjs <mcp-url> search <engine> <query>   use case A: search, open top result, extract
//   node mcp.mjs <mcp-url> read <page-url>           use case B: read a given page
//   node mcp.mjs <mcp-url> layout <page-url> <W>x<H> <wait-text> [setup-js]
//                                                    viewport check: elements past the right edge + screenshot
//
// engine: ddg | bing | brave
import { writeFileSync } from "node:fs";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

// Search result pages and the selector of organic result title links.
// Checked 2026-10-11: Mojeek answered with an ALTCHA challenge and Startpage
// with a redirect loop even from a residential IP, so they are not listed.
const ENGINES = {
  ddg: ["https://html.duckduckgo.com/html/?q=", "a.result__a"],
  bing: ["https://www.bing.com/search?q=", "li.b_algo h2 a"],
  brave: ["https://search.brave.com/search?q=", ".result-content > a"],
};

const [url, cmd, ...args] = process.argv.slice(2);
if (!url || !cmd) {
  console.error("usage: node mcp.mjs <mcp-url> tools|call|search|read ...");
  process.exit(2);
}

const client = new Client({ name: "hydra-e2e", version: "1.0.0" });
await client.connect(new StreamableHTTPClientTransport(new URL(url)));

// Text parts are returned; image parts (screenshots) are saved to mcp-image-<n>.<ext>.
async function call(name, input = {}) {
  const r = await client.callTool({ name, arguments: input });
  r.content
    .filter((c) => c.type === "image")
    .forEach((c, i) => {
      const file = `mcp-image-${i}.${c.mimeType.split("/")[1]}`;
      writeFileSync(file, Buffer.from(c.data, "base64"));
      console.error(`saved ${file}`);
    });
  const text = r.content.filter((c) => c.type === "text").map((c) => c.text).join("\n");
  if (r.isError) throw new Error(`${name}: ${text}`);
  return text;
}

// browser_evaluate returns a markdown section with the JSON result.
async function evaluate(fn) {
  const text = await call("browser_evaluate", { function: fn });
  const m = text.match(/### Result\n([\s\S]*?)(\n###|$)/);
  return JSON.parse(m ? m[1].trim().replace(/^```(json)?\n?|```$/g, "") : text);
}

/** Finds the snapshot ref of a link whose accessible name starts with `name`. */
function refOf(snapshot, name) {
  const prefix = name.trim().slice(0, 40);
  for (const line of snapshot.split("\n")) {
    const m = line.match(/link "([^"]*)".*\[ref=([^\]]+)\]/);
    if (m && m[1].startsWith(prefix)) return m[2];
  }
  return null;
}

// Retries while a navigation (e.g. after a click) replaces the document.
async function pageText(limit) {
  for (let attempt = 1; ; attempt++) {
    try {
      return await readPage(limit);
    } catch (e) {
      if (attempt >= 20 || !/context was destroyed|navigat/i.test(e.message)) throw e;
    }
  }
}

async function readPage(limit) {
  return evaluate(`() => ({ url: location.href, title: document.title, text: document.body.innerText.replace(/\\n{3,}/g, "\\n\\n").slice(0, ${limit}) })`);
}

try {
  if (cmd === "tools") {
    const { tools } = await client.listTools();
    console.log(tools.map((t) => t.name).join("\n"));
  } else if (cmd === "call") {
    console.log(await call(args[0], args[1] ? JSON.parse(args[1]) : {}));
  } else if (cmd === "search") {
    const [engine, ...words] = args;
    const [base, selector] = ENGINES[engine] ?? [];
    if (!base) throw new Error(`unknown engine ${engine}`);
    await call("browser_navigate", { url: base + encodeURIComponent(words.join(" ")) });
    const results = await evaluate(
      `() => [...document.querySelectorAll(${JSON.stringify(selector)})].slice(0, 5).map(a => ({ title: a.textContent.replace(/\\s+/g, " ").trim(), href: a.href }))`,
    );
    console.log(`== ${engine}: ${results.length} results`);
    for (const r of results) console.log(`- ${r.title}\n  ${r.href}`);
    if (!results.length) {
      console.log((await pageText(600)).text);
      throw new Error(`no results from ${engine} (captcha or layout change?)`);
    }
    // Open the top result like an agent would: by its ref in the snapshot.
    const snapshot = await call("browser_snapshot");
    const ref = refOf(snapshot, results[0].title);
    if (ref) {
      await call("browser_click", { element: results[0].title, target: ref });
    } else {
      await call("browser_navigate", { url: results[0].href });
    }
    const page = await pageText(800);
    console.log(`== opened (${ref ? `clicked ${ref}` : "navigated"}): ${page.title}\n${page.url}\n${page.text}`);
  } else if (cmd === "read") {
    await call("browser_navigate", { url: args[0] });
    const page = await pageText(1500);
    console.log(`== ${page.title}\n${page.url}\n${page.text}`);
  } else if (cmd === "layout") {
    const [page, size, waitText, setupJs] = args;
    const [width, height] = size.split("x").map(Number);
    await call("browser_resize", { width, height });
    await call("browser_navigate", { url: page });
    // "<n>s" waits n seconds (pages that render results slowly), else waits for the text.
    if (/^\d+s$/.test(waitText || "")) await call("browser_wait_for", { time: parseInt(waitText, 10) });
    else if (waitText) await call("browser_wait_for", { text: waitText });
    if (setupJs) await evaluate(setupJs);
    const r = await evaluate(`() => {
      const W = document.documentElement.clientWidth;
      const out = [...document.querySelectorAll("body *")]
        .filter((e) => getComputedStyle(e).position !== "fixed" && e.getBoundingClientRect().width > 0 && e.getBoundingClientRect().right > W + 1)
        .map((e) => e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + "." + String(e.className).split(" ")[0] + " right=" + Math.round(e.getBoundingClientRect().right));
      return { viewport: innerWidth, scrollWidth: document.documentElement.scrollWidth, overflowing: out.slice(0, 15) };
    }`);
    console.log(JSON.stringify(r, null, 2));
    await call("browser_take_screenshot", { fullPage: true });
  } else {
    throw new Error(`unknown command ${cmd}`);
  }
} catch (e) {
  console.error(`mcp: ${e.message}`);
  process.exitCode = 1;
} finally {
  await client.close();
}
