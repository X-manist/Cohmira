//! 平台请求签名实现。
//!
//! 各平台的签名参数生成，从 MediaCrawler Python 版 1:1 移植。
//! - [`bilibili`]：wbi（`w_rid`）签名 —— 纯算法（MD5+置换表）。
//! - [`xhs`]：小红书 x-s/x-t —— 移植 xhshow 纯算法。
//! - [`js_engine`]：通用 JS 引擎封装（boa_engine），供 douyin/zhihu 跑原 `libs/*.js`。
//! - [`douyin`] / [`zhihu`]：用 [`js_engine`] 跑原 JS 算 a_bogus / x-zse-96。

pub mod bilibili;
pub mod douyin;
pub mod js_engine;
pub mod xhs;
pub mod zhihu;

pub use bilibili::WbiSign;
