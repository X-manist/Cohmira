//! 浏览器自动化驱动抽象（CDP 集成点）。
//!
//! socialconnect 的发布是 RPA（点击各平台创作者中心的上传 UI），无统一开放 API。
//! 本 trait 抽象浏览器操作，由 CDP 后端（chromiumoxide，可选 feature）实现；
//! 上传器（[`crate::uploader`]）只依赖此 trait，便于单测与解耦。
//!
//! 实现策略：默认实现 [`StubDriver`]（所有方法返回未实现错误，用于编译期占位与流程测试）；
//! 真实发布由 `chromiumoxide` 后端实现（feature = "cdp"），驱动本机或 bundled Chrome。

#[cfg(feature = "cdp")]
pub mod cdp;

use async_trait::async_trait;
use std::sync::Arc;

/// 浏览器驱动：发布器所需的最小操作集（对齐 Playwright/sau 用到的 page.* 操作）。
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    /// 导航到 URL。
    async fn goto(&self, url: &str) -> anyhow::Result<()>;
    /// 当前页面 URL。
    async fn current_url(&self) -> anyhow::Result<String>;
    /// 页面是否含某文本（用于登录态/错误检测）。
    async fn is_text_present(&self, text: &str) -> anyhow::Result<bool>;
    /// 点击可见文本匹配的元素。
    async fn click_text(&self, text: &str) -> anyhow::Result<()>;
    /// 点击 CSS 选择器。
    async fn click_selector(&self, selector: &str) -> anyhow::Result<()>;
    /// 向 CSS 选择器输入文本。
    async fn fill(&self, selector: &str, value: &str) -> anyhow::Result<()>;

    /// 在现有内容后继续输入文本，不清空元素。
    async fn type_text(&self, selector: &str, value: &str) -> anyhow::Result<()> {
        self.fill(selector, value).await
    }
    /// 向 file input 设置文件路径（上传）。
    async fn set_input_files(&self, selector: &str, paths: &[String]) -> anyhow::Result<()>;

    /// 向第 `index` 个匹配的 file input 设置文件；默认仅支持第一个。
    async fn set_input_files_nth(
        &self,
        selector: &str,
        index: usize,
        paths: &[String],
    ) -> anyhow::Result<()> {
        if index == 0 {
            self.set_input_files(selector, paths).await
        } else {
            Err(err())
        }
    }
    /// 等待 URL 包含片段（登录跳转/上传完成）。
    async fn wait_for_url_contains(&self, fragment: &str) -> anyhow::Result<()>;

    /// 带调用方指定超时的 URL 等待。
    async fn wait_for_url_contains_timeout(
        &self,
        fragment: &str,
        timeout_ms: u64,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if self.current_url().await?.contains(fragment) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("等待 URL 含 {fragment:?} 超时（{timeout_ms}ms）");
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    /// 固定等待（毫秒），用于页面稳定。
    async fn sleep_ms(&self, ms: u64) -> anyhow::Result<()>;

    /// CSS 选择器是否存在。默认实现保持显式未实现，生产 CDP 驱动会覆盖。
    async fn selector_exists(&self, _selector: &str) -> anyhow::Result<bool> {
        Err(err())
    }

    /// 等待 CSS 选择器出现。
    async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if self.selector_exists(selector).await? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("等待选择器 {selector:?} 超时（{timeout_ms}ms）");
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// 获取元素属性。
    async fn attribute(&self, _selector: &str, _name: &str) -> anyhow::Result<Option<String>> {
        Err(err())
    }

    /// 在页面执行 JavaScript 并返回 JSON 值。
    async fn evaluate(&self, _script: &str) -> anyhow::Result<serde_json::Value> {
        Err(err())
    }

    /// 向当前聚焦元素发送一个按键（如 Enter / Escape / Space）。
    async fn press_key(&self, _key: &str) -> anyhow::Result<()> {
        Err(err())
    }
}

/// 占位驱动：所有方法返回未实现错误。用于无 CDP 环境下的编译期占位与流程结构测试。
pub struct StubDriver;

#[async_trait]
impl BrowserDriver for StubDriver {
    async fn goto(&self, _url: &str) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "StubDriver: 需要 chromiumoxide CDP 后端（feature=cdp）"
        ))
    }
    async fn current_url(&self) -> anyhow::Result<String> {
        Err(err())
    }
    async fn is_text_present(&self, _text: &str) -> anyhow::Result<bool> {
        Err(err())
    }
    async fn click_text(&self, _text: &str) -> anyhow::Result<()> {
        Err(err())
    }
    async fn click_selector(&self, _selector: &str) -> anyhow::Result<()> {
        Err(err())
    }
    async fn fill(&self, _selector: &str, _value: &str) -> anyhow::Result<()> {
        Err(err())
    }
    async fn set_input_files(&self, _selector: &str, _paths: &[String]) -> anyhow::Result<()> {
        Err(err())
    }
    async fn wait_for_url_contains(&self, _fragment: &str) -> anyhow::Result<()> {
        Err(err())
    }
    async fn sleep_ms(&self, ms: u64) -> anyhow::Result<()> {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(())
    }
}

fn err() -> anyhow::Error {
    anyhow::anyhow!("StubDriver: 需要 chromiumoxide CDP 后端（feature=cdp）")
}

/// 让上传器可以持有 `Arc<CdpBrowser>`，调用方同时保留会话以便发布后导出 Cookie 并关闭浏览器。
#[async_trait]
impl<T> BrowserDriver for Arc<T>
where
    T: BrowserDriver + ?Sized,
{
    async fn goto(&self, url: &str) -> anyhow::Result<()> {
        (**self).goto(url).await
    }

    async fn current_url(&self) -> anyhow::Result<String> {
        (**self).current_url().await
    }

    async fn is_text_present(&self, text: &str) -> anyhow::Result<bool> {
        (**self).is_text_present(text).await
    }

    async fn click_text(&self, text: &str) -> anyhow::Result<()> {
        (**self).click_text(text).await
    }

    async fn click_selector(&self, selector: &str) -> anyhow::Result<()> {
        (**self).click_selector(selector).await
    }

    async fn fill(&self, selector: &str, value: &str) -> anyhow::Result<()> {
        (**self).fill(selector, value).await
    }

    async fn type_text(&self, selector: &str, value: &str) -> anyhow::Result<()> {
        (**self).type_text(selector, value).await
    }

    async fn set_input_files(&self, selector: &str, paths: &[String]) -> anyhow::Result<()> {
        (**self).set_input_files(selector, paths).await
    }

    async fn set_input_files_nth(
        &self,
        selector: &str,
        index: usize,
        paths: &[String],
    ) -> anyhow::Result<()> {
        (**self).set_input_files_nth(selector, index, paths).await
    }

    async fn wait_for_url_contains(&self, fragment: &str) -> anyhow::Result<()> {
        (**self).wait_for_url_contains(fragment).await
    }

    async fn wait_for_url_contains_timeout(
        &self,
        fragment: &str,
        timeout_ms: u64,
    ) -> anyhow::Result<()> {
        (**self)
            .wait_for_url_contains_timeout(fragment, timeout_ms)
            .await
    }

    async fn sleep_ms(&self, ms: u64) -> anyhow::Result<()> {
        (**self).sleep_ms(ms).await
    }

    async fn selector_exists(&self, selector: &str) -> anyhow::Result<bool> {
        (**self).selector_exists(selector).await
    }

    async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> anyhow::Result<()> {
        (**self).wait_for_selector(selector, timeout_ms).await
    }

    async fn attribute(&self, selector: &str, name: &str) -> anyhow::Result<Option<String>> {
        (**self).attribute(selector, name).await
    }

    async fn evaluate(&self, script: &str) -> anyhow::Result<serde_json::Value> {
        (**self).evaluate(script).await
    }

    async fn press_key(&self, key: &str) -> anyhow::Result<()> {
        (**self).press_key(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_driver_errors_except_sleep() {
        let d = StubDriver;
        assert!(d.goto("x").await.is_err());
        // sleep 应可用（不依赖 CDP）
        assert!(d.sleep_ms(1).await.is_ok());
    }

    #[tokio::test]
    async fn driver_is_object_safe() {
        // trait object 可构造（证明 trait 定义可用）。
        let d: Box<dyn BrowserDriver> = Box::new(StubDriver);
        assert!(d.current_url().await.is_err());
    }
}
