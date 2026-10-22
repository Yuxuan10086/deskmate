//! 摄像头人脸在位检测：MediaCapture 抓帧 → 系统内置 FaceDetector 推理。
//!
//! 使用 Windows 自带能力（Windows.Media.Capture / Windows.Media.FaceAnalysis），
//! 无需随包分发 ONNX 模型与推理运行时，安装包体积不变、无额外依赖。
//! 摄像头无设备、被占用或无访问权限时返回 `Unavailable`，由上层提示用户。

use windows_collections::IMapView;

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Capture::Frames::{
    MediaFrameReader, MediaFrameReaderAcquisitionMode, MediaFrameReaderStartStatus,
    MediaFrameSource, MediaFrameSourceGroup, MediaFrameSourceKind,
};
use windows::Media::Capture::{
    MediaCapture, MediaCaptureInitializationSettings, MediaCaptureMemoryPreference,
    MediaCaptureSharingMode, MediaStreamType, StreamingCaptureMode,
};
use windows::Media::FaceAnalysis::FaceDetector;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::error::{AppError, AppResult};

/// 确保当前线程已初始化 COM/WinRT（WinRT 激活与完成回调分发的前提）。
///
/// 注意：WinRT 异步结果**不能用阻塞式 `GetResults()` 获取**——在 MTA 线程上调用会
/// 立即返回 0x8000000E「在意外的时间调用了方法」（实测：在位检测因此从未产出过任何判定）。
/// 正确做法是 `windows_future` 的 `join()`（注册完成回调 + 等待事件）。
fn ensure_thread_mta() {
    unsafe {
        // 返回 S_FALSE（已初始化）/ RPC_E_CHANGED_MODE（已是其它模式）都无需处理
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// 单帧人脸检测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceSignal {
    /// 画面中检测到人脸
    Present,
    /// 画面正常但未检测到人脸
    Absent,
    /// 摄像头不可用（无设备 / 被占用 / 无访问权限）
    Unavailable,
}

/// 把 Windows API 错误包装成带中文上下文的 AppError。
fn err(context: &'static str) -> impl Fn(windows::core::Error) -> AppError {
    move |e| AppError::new(format!("{context}：{e}"))
}

/// 摄像头人脸信号源：保持一路常驻抓帧会话，按需取最新帧做一次检测。
///
/// 句柄不跨线程移动：在检测线程内创建、在该线程内使用与释放。
pub struct CameraFaceSource {
    /// 必须保持存活，否则帧读取器会失效。
    _capture: MediaCapture,
    reader: MediaFrameReader,
    detector: FaceDetector,
}

impl CameraFaceSource {
    /// 打开默认彩色摄像头并启动常驻抓帧会话。
    pub fn open() -> AppResult<Self> {
        crate::win_audio::ensure_com()?;
        ensure_thread_mta();

        let capture = MediaCapture::new().map_err(err("创建摄像头会话失败"))?;
        let settings =
            MediaCaptureInitializationSettings::new().map_err(err("创建采集设置失败"))?;
        settings
            .SetStreamingCaptureMode(StreamingCaptureMode::Video)
            .map_err(err("设置采集模式失败"))?;
        // 以 CPU 内存方式取帧，才能拿到 SoftwareBitmap 交给检测器
        settings
            .SetMemoryPreference(MediaCaptureMemoryPreference::Cpu)
            .map_err(err("设置内存偏好失败"))?;
        // 只读共享：不与其它正在使用摄像头的程序争抢独占
        settings
            .SetSharingMode(MediaCaptureSharingMode::SharedReadOnly)
            .map_err(err("设置共享模式失败"))?;
        // MSDN 硬性要求：访问 FrameSources() 前必须指定一个含彩色源的源组，
        // 否则 InitializeWithSettingsAsync 后取帧源报 0xC00D36B6 (MF_E_NOT_INITIALIZED)
        let group = pick_source_group()?;
        settings
            .SetSourceGroup(&group)
            .map_err(err("绑定摄像头源组失败"))?;
        capture
            .InitializeWithSettingsAsync(&settings)
            .map_err(err("初始化摄像头失败，请确认已连接摄像头并允许桌面应用访问相机"))?
            .join()
            .map_err(err("初始化摄像头失败，请确认已连接摄像头并允许桌面应用访问相机"))?;

        let sources = capture.FrameSources().map_err(err("枚举摄像头帧源失败"))?;
        let source = pick_color_source(&sources)?;
        let reader = capture
            .CreateFrameReaderAsync(&source)
            .map_err(err("创建帧读取器失败"))?
            .join()
            .map_err(err("创建帧读取器失败"))?;
        reader
            .SetAcquisitionMode(MediaFrameReaderAcquisitionMode::Realtime)
            .map_err(err("设置帧读取模式失败"))?;
        let status = reader
            .StartAsync()
            .map_err(err("启动帧读取失败"))?
            .join()
            .map_err(err("启动帧读取失败"))?;
        if status != MediaFrameReaderStartStatus::Success {
            return Err(AppError::new(format!(
                "摄像头启动失败（{}），可能被其它程序独占",
                status_text(status)
            )));
        }

        let detector = FaceDetector::CreateAsync()
            .map_err(err("创建人脸检测器失败[激活]"))?
            .join()
            .map_err(err("创建人脸检测器失败[等待]"))?;

        Ok(Self {
            _capture: capture,
            reader,
            detector,
        })
    }

    /// 取最新一帧做一次人脸检测；暂无可用帧时返回 None（本轮跳过，不参与判定）。
    pub fn detect(&self) -> Option<FaceSignal> {
        let frame = self.reader.TryAcquireLatestFrame().ok()?;
        let bitmap = frame
            .VideoMediaFrame()
            .and_then(|v| v.SoftwareBitmap())
            .ok()?;
        let gray = match to_detectable(&bitmap) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("[face] {e:?}");
                return Some(FaceSignal::Unavailable);
            }
        };
        let faces = match self.detector.DetectFacesAsync(&gray) {
            Ok(op) => match op.join() {
                Ok(v) => v,
                Err(_) => return Some(FaceSignal::Unavailable),
            },
            Err(_) => return Some(FaceSignal::Unavailable),
        };
        match faces.Size() {
            Ok(0) => Some(FaceSignal::Absent),
            Ok(_) => Some(FaceSignal::Present),
            Err(_) => Some(FaceSignal::Unavailable),
        }
    }

    /// 停止抓帧并释放摄像头（忽略失败：对象随即析构）。
    pub fn close(&self) {
        let _ = self.reader.StopAsync();
    }
}

/// 临时打开摄像头探测一次（供设置页「测试摄像头」按钮使用）。
///
/// 摄像头刚启动时首帧可能尚未就绪，这里最多重试 `ATTEMPTS` 次。
pub fn probe_once() -> AppResult<FaceSignal> {
    const ATTEMPTS: u32 = 8;

    let source = CameraFaceSource::open()?;
    let mut result = None;
    for _ in 0..ATTEMPTS {
        if let Some(signal) = source.detect() {
            result = Some(signal);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    source.close();
    result.ok_or_else(|| AppError::new("摄像头已打开但未能取到画面，请稍后重试"))
}

/// 枚举全部摄像头源组，返回第一个含彩色视频源的组（一般即默认摄像头）。
fn pick_source_group() -> AppResult<MediaFrameSourceGroup> {
    let groups = MediaFrameSourceGroup::FindAllAsync()
        .map_err(err("枚举摄像头失败"))?
        .join()
        .map_err(err("枚举摄像头失败"))?;
    let count = groups.Size().unwrap_or(0);
    for i in 0..count {
        let Ok(group) = groups.GetAt(i) else { continue };
        let Ok(infos) = group.SourceInfos() else {
            continue;
        };
        let n = infos.Size().unwrap_or(0);
        for j in 0..n {
            let Ok(info) = infos.GetAt(j) else { continue };
            let is_color = info
                .SourceKind()
                .map(|k| k == MediaFrameSourceKind::Color)
                .unwrap_or(false);
            let is_video = info
                .MediaStreamType()
                .map(|t| t == MediaStreamType::VideoRecord || t == MediaStreamType::VideoPreview)
                .unwrap_or(false);
            if is_color && is_video {
                return Ok(group);
            }
        }
    }
    Err(AppError::new(
        "未找到可用的彩色摄像头，请确认摄像头已连接且未被其它程序独占",
    ))
}

/// 从帧源集合中挑一路彩色视频源（多数摄像头的 Color 源即视频流）。
fn pick_color_source(
    sources: &IMapView<windows::core::HSTRING, MediaFrameSource>,
) -> AppResult<MediaFrameSource> {
    let iter = sources.First().map_err(err("遍历帧源失败"))?;
    while iter.HasCurrent().unwrap_or(false) {
        let pair = iter.Current().map_err(err("读取帧源失败"))?;
        let source = pair.Value().map_err(err("读取帧源失败"))?;
        if let Ok(info) = source.Info() {
            let is_color = info
                .SourceKind()
                .map(|k| k == MediaFrameSourceKind::Color)
                .unwrap_or(false);
            let is_video = info
                .MediaStreamType()
                .map(|t| t == MediaStreamType::VideoRecord || t == MediaStreamType::VideoPreview)
                .unwrap_or(false);
            if is_color && is_video {
                return Ok(source);
            }
        }
        if iter.MoveNext().is_err() {
            break;
        }
    }
    Err(AppError::new(
        "未找到可用的彩色摄像头，请确认摄像头已连接且未被其它程序独占",
    ))
}

/// 归一化为检测器支持的像素格式（Gray8 / Nv12），已是则直接复用。
fn to_detectable(src: &SoftwareBitmap) -> AppResult<SoftwareBitmap> {
    let format = src.BitmapPixelFormat().map_err(err("读取帧格式失败"))?;
    if format == BitmapPixelFormat::Gray8 || format == BitmapPixelFormat::Nv12 {
        return Ok(src.clone());
    }
    SoftwareBitmap::Convert(src, BitmapPixelFormat::Gray8).map_err(err("转换帧格式失败"))
}

fn status_text(status: MediaFrameReaderStartStatus) -> &'static str {
    match status {
        MediaFrameReaderStartStatus::UnknownFailure => "未知错误",
        MediaFrameReaderStartStatus::DeviceNotAvailable => "设备不可用",
        MediaFrameReaderStartStatus::OutputFormatNotSupported => "输出格式不支持",
        MediaFrameReaderStartStatus::ExclusiveControlNotAvailable => "设备已被独占",
        _ => "状态异常",
    }
}
