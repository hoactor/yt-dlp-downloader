# yt-dlp 다운로더

YouTube 영상/오디오/자막 다운로더 데스크톱 앱 (macOS)

## 기술 스택

- **프론트엔드**: React 19 + TypeScript + Vite 7
- **백엔드**: Tauri v2 (Rust)
- **외부 의존**: yt-dlp (brew install), ffmpeg, Node.js (yt-dlp YouTube 파싱용)

## 프로젝트 구조

```
src/App.tsx          # React UI (단일 컴포넌트)
src/App.css          # 스타일
src-tauri/src/main.rs # Rust 백엔드 (yt-dlp 실행, 진행률 파싱, 자막 변환)
src-tauri/tauri.conf.json # Tauri 설정
```

## 개발 명령어

```bash
npm run tauri dev    # 개발 서버 실행
npm run tauri build  # 프로덕션 빌드 (.app 생성)
npm run dev          # 프론트엔드만 실행 (Vite)
npm run build        # 프론트엔드 빌드
```

## 다운로드 모드

| 모드 | ID | 설명 |
|------|----|------|
| WAV | `audio_wav` | 무손실 오디오 추출 |
| MP3 | `audio_mp3` | 경량 오디오 추출 |
| 최고 화질 | `video_best` | 원본 최대 해상도 영상 |
| 1080p | `video_1080` | Full HD 고정 영상 |
| 자막만 | `sub_only` | 자막 파일만 (VTT + TXT 자동 변환) |
| 영상+자막 | `video_sub` | 영상 + 별도 자막 파일 |
| 하드코딩 | `video_hardsub` | 자막 내장(embed) 영상 |

## 주요 동작

- 영상별 폴더 자동 생성: `저장경로/영상제목/영상제목.확장자`
- VTT 자막 다운로드 시 자동으로 TXT 순수 텍스트 변환
- 설정 파일: `~/Library/Application Support/yt-dlp-downloader/settings.json`
- 기본 저장 경로: `/Users/honamgung/Documents/00_유튜브_영상_다운로드`
- yt-dlp, ffmpeg, node 경로 자동 탐지 (`/opt/homebrew/bin`, `/usr/local/bin`)
- 다운로드 취소: SIGTERM으로 yt-dlp 프로세스 종료

## 참고

- `video_sub` 모드는 자막을 영상에 입히지 않고 별도 파일로 저장하지만, 재생기(VLC 등)가 같은 폴더의 .vtt를 자동 로드하면 자막이 표시될 수 있음
- `video_hardsub`의 `--embed-subs`는 소프트 자막(soft sub)으로, 플레이어에서 끄기 가능
