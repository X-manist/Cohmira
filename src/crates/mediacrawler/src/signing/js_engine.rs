//! 通用 JS 引擎封装（boa_engine 0.20）。
//!
//! 供 douyin/zhihu 跑平台原始混淆 JS（`libs/douyin.js` / `libs/zhihu.js`）算签名。
//! boa_engine 是纯 Rust JS 引擎，无 C 依赖，利于桌面打包。
//!
//! 用法：
//! ```ignore
//! let mut eng = JsEngine::new()?;
//! eng.load(include_str!("douyin.js"))?;
//! let a_bogus = eng.call("sign", &["query".into(), "ua".into()])?;
//! ```
//!
//! 注：平台 JS 可能引用 `window`/`navigator` 等浏览器全局；[`JsEngine::new`] 注入最小 shim。

use anyhow::Context as _;

/// boa JS 引擎包装。
pub struct JsEngine {
    ctx: boa_engine::Context,
}

impl JsEngine {
    /// 创建引擎并注入最小浏览器全局 shim。
    pub fn new() -> anyhow::Result<Self> {
        let mut ctx = boa_engine::Context::default();

        let shim = r#"
            var navigator = navigator || { userAgent: "Mozilla/5.0", platform: "MacIntel", language: "zh-CN" };
            var window = (typeof window !== "undefined") ? window : globalThis;
            try { window.navigator = navigator; } catch(e) {}
            var document = (typeof document !== "undefined") ? document : { cookie: "", createElement: function(){return {}}, location: { href: "" } };
        "#;
        ctx.eval(boa_engine::Source::from_bytes(shim))
            .map_err(|e| anyhow::anyhow!("js shim eval failed: {e}"))?;

        Ok(Self { ctx })
    }

    /// 加载（eval）一段 JS，用于定义函数/变量。可多次调用累积。
    pub fn load(&mut self, js: &str) -> anyhow::Result<()> {
        self.ctx
            .eval(boa_engine::Source::from_bytes(js))
            .map_err(|e| anyhow::anyhow!("js load failed: {e}"))?;
        Ok(())
    }

    /// 调用已定义的全局函数 `fn_name(args...)`，返回其字符串结果。
    pub fn call(&mut self, fn_name: &str, args: &[String]) -> anyhow::Result<String> {
        let global = self.ctx.global_object();
        let func = global
            .get(boa_engine::JsString::from(fn_name), &mut self.ctx)
            .map_err(|e| anyhow::anyhow!("js get fn {fn_name} failed: {e}"))?;
        let func_obj = func
            .as_object()
            .with_context(|| format!("js: {fn_name} 不是可调用对象"))?;

        let js_args: Vec<boa_engine::JsValue> = args
            .iter()
            .map(|a| boa_engine::JsValue::String(boa_engine::JsString::from(a.as_str())))
            .collect();

        let result = func_obj
            .call(&boa_engine::JsValue::Undefined, &js_args, &mut self.ctx)
            .map_err(|e| anyhow::anyhow!("js call {fn_name} failed: {e}"))?;

        let s = result
            .to_string(&mut self.ctx)
            .map_err(|e| anyhow::anyhow!("js result to_string failed: {e}"))?;
        Ok(s.to_std_string_escaped())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_and_call_basic() {
        let mut eng = JsEngine::new().unwrap();
        eng.load("function add(a, b){ return Number(a) + Number(b); }")
            .unwrap();
        let out = eng.call("add", &["2".into(), "3".into()]).unwrap();
        assert_eq!(out.trim().parse::<i64>().unwrap(), 5);
    }

    #[test]
    fn shim_globals_present() {
        let mut eng = JsEngine::new().unwrap();
        eng.load("function ua(){ return navigator.userAgent; }")
            .unwrap();
        let out = eng.call("ua", &[]).unwrap();
        assert!(out.contains("Mozilla"), "out={out}");
    }
}
