import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

const APP_VERSION = "1.0.0";
const BUILD_DATE = "2026-03-27";

type Tab = "audio" | "video" | "subtitle";

interface ToastState {
  message: string;
  type: "success" | "error";
}

const AUDIO_OPTIONS = [
  { id: "audio_wav", label: "WAV", desc: "무손실, 편집용" },
  { id: "audio_mp3", label: "MP3", desc: "경량, 공유용" },
];

const VIDEO_OPTIONS = [
  { id: "video_best", label: "최고 화질", desc: "원본 최대 해상도" },
  { id: "video_1080", label: "1080p", desc: "Full HD 고정" },
];

const SUB_OPTIONS = [
  { id: "sub_only", label: "자막만", desc: "자동생성 포함" },
  { id: "video_sub", label: "영상+자막", desc: "별도 자막 파일" },
  { id: "video_hardsub", label: "하드코딩", desc: "자막 내장" },
];

function App() {
  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [filename, setFilename] = useState("");
  const [tab, setTab] = useState<Tab>("audio");
  const [selectedMode, setSelectedMode] = useState("audio_wav");
  const [outputDir, setOutputDir] = useState("");
  const [ytdlpVersion, setYtdlpVersion] = useState("");
  const [ytdlpMissing, setYtdlpMissing] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState(0);
  const [toast, setToast] = useState<ToastState | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  const titleTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    invoke<string>("check_ytdlp")
      .then((v) => setYtdlpVersion(v))
      .catch(() => setYtdlpMissing(true));

    invoke<string>("get_default_download_dir")
      .then((dir) => setOutputDir(dir))
      .catch(() => {});

    const unlisten = listen<{ percent: number; status: string }>(
      "download-progress",
      (event) => {
        setProgress(event.payload.percent);
      }
    );

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const showToast = useCallback((message: string, type: "success" | "error") => {
    setToast({ message, type });
    setTimeout(() => setToast(null), 3000);
  }, []);

  const handleUrlChange = (value: string) => {
    setUrl(value);
    setTitle("");
    if (titleTimeout.current) clearTimeout(titleTimeout.current);
    if (value.trim()) {
      titleTimeout.current = setTimeout(() => {
        invoke<string>("fetch_title", { url: value })
          .then((t) => setTitle(t))
          .catch(() => {});
      }, 800);
    }
  };

  const handleTabChange = (newTab: Tab) => {
    setTab(newTab);
    if (newTab === "audio") setSelectedMode("audio_wav");
    else if (newTab === "video") setSelectedMode("video_best");
    else setSelectedMode("sub_only");
  };

  const handleDownload = async () => {
    if (!url.trim()) {
      showToast("URL을 입력하세요.", "error");
      return;
    }
    setDownloading(true);
    setProgress(0);
    try {
      const result = await invoke<string>("download", {
        url: url.trim(),
        mode: selectedMode,
        filename: filename.trim() || null,
        outputDir: outputDir,
      });
      showToast(result, "success");
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setDownloading(false);
    }
  };

  const handleCancel = async () => {
    try {
      await invoke("cancel_download");
      setDownloading(false);
      setProgress(0);
      showToast("다운로드가 취소되었습니다.", "error");
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  const currentOptions =
    tab === "audio" ? AUDIO_OPTIONS : tab === "video" ? VIDEO_OPTIONS : SUB_OPTIONS;

  return (
    <div className="app">
      {toast && <div className={`toast ${toast.type}`}>{toast.message}</div>}

      {/* About 모달 */}
      {showAbout && (
        <div className="modal-overlay" onClick={() => setShowAbout(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <span className="modal-icon">🟠</span>
              <span className="modal-title">yt-dlp 다운로더</span>
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">앱 버전</span>
                <span className="modal-value">v{APP_VERSION}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">빌드 날짜</span>
                <span className="modal-value">{BUILD_DATE}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">yt-dlp</span>
                <span className="modal-value">
                  {ytdlpVersion ? `v${ytdlpVersion}` : "미설치"}
                </span>
              </div>
              <div className="modal-row">
                <span className="modal-label">프레임워크</span>
                <span className="modal-value">Tauri v2 + React</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">저장 경로</span>
                <span className="modal-value mono">{outputDir}</span>
              </div>
              <div className="modal-divider" />
              <div className="modal-features">
                <div className="modal-feature-title">지원 기능</div>
                <div className="modal-feature">오디오 추출 (WAV / MP3)</div>
                <div className="modal-feature">영상 다운로드 (최고화질 / 1080p)</div>
                <div className="modal-feature">자막 (VTT + 텍스트 자동 추출)</div>
                <div className="modal-feature">자막 하드코딩 내장</div>
                <div className="modal-feature">영상별 폴더 자동 생성</div>
              </div>
            </div>
            <button className="modal-close-btn" onClick={() => setShowAbout(false)}>
              닫기
            </button>
          </div>
        </div>
      )}

      <div className="header" onClick={() => setShowAbout(true)} style={{ cursor: "pointer" }}>
        <span className="header-icon">🟠</span>
        <h1>yt-dlp 다운로더</h1>
        <span className="header-version">v{APP_VERSION}</span>
      </div>

      {ytdlpMissing && (
        <div className="warning-banner">
          yt-dlp가 설치되어 있지 않습니다. 터미널에서 <code>brew install yt-dlp</code> 를
          실행하세요.
        </div>
      )}

      <div className="input-group">
        <input
          className="input-field"
          placeholder="YouTube URL을 붙여넣으세요"
          value={url}
          onChange={(e) => handleUrlChange(e.target.value)}
          disabled={downloading}
        />
        <div className="title-preview">{title}</div>
        <input
          className="input-field"
          placeholder="파일명 (비우면 원본 제목 사용)"
          value={filename}
          onChange={(e) => setFilename(e.target.value)}
          disabled={downloading}
        />
      </div>

      <div className="tabs">
        {(["audio", "video", "subtitle"] as Tab[]).map((t) => (
          <button
            key={t}
            className={`tab ${tab === t ? "active" : ""}`}
            onClick={() => handleTabChange(t)}
            disabled={downloading}
          >
            {t === "audio" ? "오디오" : t === "video" ? "영상" : "자막"}
          </button>
        ))}
      </div>

      <div className={`options-grid ${tab === "subtitle" ? "three-col" : ""}`}>
        {currentOptions.map((opt) => (
          <div
            key={opt.id}
            className={`option-card ${selectedMode === opt.id ? "selected" : ""}`}
            onClick={() => !downloading && setSelectedMode(opt.id)}
          >
            <div className="label">{opt.label}</div>
            <div className="desc">{opt.desc}</div>
          </div>
        ))}
      </div>

      {downloading ? (
        <>
          <div className="progress-container">
            <div className="progress-bar-bg">
              <div
                className={`progress-bar-fill ${progress >= 100 ? "done" : ""}`}
                style={{ width: `${progress}%` }}
              />
            </div>
            <div className="progress-text">{progress.toFixed(1)}%</div>
          </div>
          <button className="download-btn cancel" onClick={handleCancel}>
            취소
          </button>
        </>
      ) : (
        <button
          className="download-btn"
          onClick={handleDownload}
          disabled={ytdlpMissing || !url.trim()}
        >
          다운로드
        </button>
      )}

      <div className="footer">
        <div className="path-row">
          <span className="path-label">저장 경로:</span>
          <span className="path-value">{outputDir}</span>
          <button
            className="path-change-btn"
            onClick={() => {
              /* Phase 3에서 dialog 플러그인으로 구현 */
            }}
          >
            변경
          </button>
        </div>
        {ytdlpVersion && <div className="version-text">yt-dlp v{ytdlpVersion}</div>}
      </div>
    </div>
  );
}

export default App;
