use std::sync::Arc;

use anyhow::Result;
use rquickjs::context::EvalOptions;
use rquickjs::prelude::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Function, Promise, Value, async_with};

use crate::catalog::Catalog;
use crate::client::ClientPool;
use crate::transpile;

/// JS sandbox that executes agent-written code with proxied MCP tool calls.
pub struct Sandbox {
    #[allow(dead_code)]
    rt: AsyncRuntime,
    ctx: AsyncContext,
    pool: Arc<ClientPool>,
    catalog: Arc<Catalog>,
}

fn eval_opts() -> EvalOptions {
    let mut opts = EvalOptions::default();
    opts.global = true;
    opts.strict = false;
    opts.promise = false;
    opts
}

const CONSOLE_SHIM: &str = r#"
const console = {
  _write(level, args) {
    const msg = args.map(a => {
      if (typeof a === 'string') return a;
      try { return JSON.stringify(a); } catch { return String(a); }
    }).join(' ');
    __stderr(level + ': ' + msg);
  },
  log(...args)   { this._write('LOG', args); },
  info(...args)  { this._write('INFO', args); },
  warn(...args)  { this._write('WARN', args); },
  error(...args) { this._write('ERROR', args); },
  debug(...args) { this._write('DEBUG', args); },
};
"#;

impl Sandbox {
    pub async fn new(pool: Arc<ClientPool>, catalog: Arc<Catalog>) -> Result<Self> {
        let rt = AsyncRuntime::new()?;
        rt.set_memory_limit(64 * 1024 * 1024).await;
        let ctx = AsyncContext::full(&rt).await?;

        async_with!(ctx => |ctx| {
            let stderr_fn = Function::new(ctx.clone(), |msg: String| {
                eprintln!("[js] {msg}");
            })
            .map_err(|e| anyhow::anyhow!("failed to create __stderr: {e}"))?;

            ctx.globals()
                .set("__stderr", stderr_fn)
                .map_err(|e| anyhow::anyhow!("failed to set __stderr: {e}"))?;

            ctx.eval::<(), _>(CONSOLE_SHIM)
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("failed to install console shim: {e}"))?;

            Ok::<_, anyhow::Error>(())
        })
        .await?;

        Ok(Self { rt, ctx, pool, catalog })
    }

    /// Execute TypeScript code that filters the tool catalog (search_code tool).
    pub async fn search_code(&self, code: &str) -> Result<serde_json::Value> {
        let catalog_json_str = serde_json::to_string(&self.catalog.to_json_value())?;
        let code = transpile_agent_code(code, &self.catalog.type_declarations())?;

        let result = async_with!(self.ctx => |ctx| {
            let tools_val: Value = ctx.json_parse(catalog_json_str)
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("failed to parse catalog: {e}"))?;

            ctx.globals()
                .set("tools", tools_val)
                .map_err(|e| anyhow::anyhow!("failed to set tools: {e}"))?;

            let wrapped = format!("(async () => {{ {code} }})()", code = code);

            let promise: Promise = ctx.eval_with_options(wrapped, eval_opts())
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("JS eval error: {e}"))?;

            let result: Value = promise
                .into_future::<Value>()
                .await
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("JS promise rejected: {e}"))?;

            stringify_result(&ctx, result)
        })
        .await?;

        Ok(result)
    }

    /// Execute TypeScript code that calls tools across servers (execute tool).
    pub async fn execute(&self, code: &str) -> Result<serde_json::Value> {
        let pool = self.pool.clone();
        let catalog = self.catalog.clone();
        let code = transpile_agent_code(code, &self.catalog.type_declarations())?;

        let result = async_with!(self.ctx => |ctx| {
            let pool_ref = pool.clone();
            let call_tool_fn = Function::new(
                ctx.clone(),
                Async({
                    let pool = pool_ref.clone();
                    move |server: String, tool: String, params_json: String| {
                        let pool_inner = pool.clone();
                        async move {
                            let params: serde_json::Value =
                                serde_json::from_str(&params_json)
                                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                            match pool_inner.call_tool(&server, &tool, params).await {
                                Ok(call_result) => serde_json::to_string(&call_result)
                                    .unwrap_or_else(|_| "null".to_owned()),
                                Err(e) => {
                                    format!(
                                        r#"{{"error":"{}"}}"#,
                                        e.to_string().replace('"', "\\\"")
                                    )
                                }
                            }
                        }
                    }
                }),
            )
            .map_err(|e| anyhow::anyhow!("failed to create __call_tool: {e}"))?;

            ctx.globals()
                .set("__call_tool", call_tool_fn)
                .map_err(|e| anyhow::anyhow!("failed to set __call_tool: {e}"))?;

            let mut setup = String::new();

            let mut server_names: Vec<&str> = catalog
                .entries()
                .iter()
                .map(|e| e.server.as_str())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            server_names.sort();

            for name in &server_names {
                let js_name = name.replace('-', "_");
                setup.push_str(&format!(
                    r#"const {js_name} = new Proxy({{}}, {{
  get(_, tool) {{
    return async (args = {{}}) => {{
      const resultJson = await __call_tool("{name}", tool, JSON.stringify(args));
      try {{ return JSON.parse(resultJson); }} catch {{ return resultJson; }}
    }};
  }}
}});
"#,
                    js_name = js_name,
                    name = name,
                ));
            }

            let catalog_json_str =
                serde_json::to_string(&catalog.to_json_value()).unwrap_or_else(|_| "[]".to_owned());
            setup.push_str(&format!("const tools = {};", catalog_json_str));

            let wrapped =
                format!("(async () => {{ {setup}\n{code} }})()", setup = setup, code = code);

            let promise: Promise = ctx.eval_with_options(wrapped, eval_opts())
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("JS eval error: {e}"))?;

            let result: Value = promise
                .into_future::<Value>()
                .await
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("JS promise rejected: {e}"))?;

            stringify_result(&ctx, result)
        })
        .await?;

        Ok(result)
    }
}

fn stringify_result<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: Value<'js>,
) -> Result<serde_json::Value> {
    let json_rq_str = ctx
        .json_stringify(value)
        .catch(ctx)
        .map_err(|e| anyhow::anyhow!("failed to stringify: {e}"))?;

    let json_std_str = match json_rq_str {
        Some(s) => s.to_string().map_err(|e| anyhow::anyhow!("string conversion: {e}"))?,
        None => "null".to_owned(),
    };

    serde_json::from_str(&json_std_str).map_err(|e| anyhow::anyhow!("JSON parse error: {e}"))
}

/// Prepend type declarations, wrap in async function, transpile TS→JS, extract body.
fn transpile_agent_code(code: &str, type_decls: &str) -> Result<String> {
    let ts_source = format!("{type_decls}\nasync function __agent__() {{\n{code}\n}}",);
    let js = transpile::ts_to_js(&ts_source)
        .map_err(|e| anyhow::anyhow!("TypeScript transpile error: {e}"))?;

    let body = if let Some(start) = js.find("async function __agent__()") {
        let after_fn = &js[start..];
        if let Some(open) = after_fn.find('{') {
            let inner = &after_fn[open + 1..];
            if let Some(close) = inner.rfind('}') {
                inner[..close].trim().to_string()
            } else {
                inner.trim().to_string()
            }
        } else {
            js
        }
    } else {
        js
    };

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    async fn test_sandbox() -> Sandbox {
        let (pool, catalog) = ClientPool::connect(HashMap::new()).await.unwrap();
        Sandbox::new(Arc::new(pool), Arc::new(catalog)).await.unwrap()
    }

    #[tokio::test]
    async fn test_execute_basic() {
        let sandbox = test_sandbox().await;
        let result = sandbox.execute("return 1 + 2;").await.unwrap();
        assert_eq!(result, serde_json::json!(3));
    }

    #[tokio::test]
    async fn test_search_code_filters_tools() {
        let sandbox = test_sandbox().await;
        let result = sandbox.search_code("return tools.filter(t => t.name.length > 0);").await.unwrap();
        assert!(result.is_array());
    }

    #[tokio::test]
    async fn test_call_tool_nonexistent_server_returns_error() {
        let sandbox = test_sandbox().await;
        let result = sandbox
            .execute(r#"
                const r = await __call_tool("no_such_server", "some_tool", "{}");
                return JSON.parse(r);
            "#)
            .await
            .unwrap();
        assert!(result.get("error").is_some());
    }
}
