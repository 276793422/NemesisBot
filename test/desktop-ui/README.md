# NemesisBot Desktop UI - Prototype

This is a prototype desktop UI for NemesisBot using:
- **Go** with webview library
- **OpenFang-style** CSS theme
- **Embedded** static assets (no external files needed)
- **Alpine.js-style** vanilla JavaScript

## Features

- ✅ Desktop window using system WebView
- ✅ Embedded HTML/CSS/JS assets
- ✅ OpenFang-inspired dark/light theme
- ✅ Chat interface prototype
- ✅ Page navigation (Chat, Overview, Logs, Settings)
- ✅ Go ↔ JavaScript bindings

## Building

### Prerequisites

**Windows**:
- WebView2 Runtime (usually pre-installed on Windows 10/11)
- Go 1.23+
- GCC (MinGW) or MSVC

**macOS**:
- Xcode Command Line Tools
- Go 1.23+

**Linux**:
- libwebkit2gtk-4.0-dev
- Go 1.23+

### Install Dependencies

```bash
go mod download
```

### Build

```bash
# Windows
go build -o desktop-ui.exe

# Linux/macOS
go build -o desktop-ui
```

## Running

```bash
# Launch desktop UI
./desktop-ui desktop

# Or launch web server only
./desktop-ui web
```

## Project Structure

```
desktop-ui/
├── main.go           # Main entry point
├── go.mod            # Go module definition
├── static/           # Embedded assets (go:embed)
│   ├── css/
│   │   ├── theme.css      # OpenFang-style theme
│   │   └── layout.css     # Layout utilities
│   ├── js/
│   │   └── app.js         # Frontend logic
│   └── index.html         # Main HTML
└── README.md          # This file
```

## Go ↔ JavaScript Bindings

### Calling Go from JavaScript

```javascript
// In JavaScript
const version = window.callGo('getVersion');
const config = window.callGo('getConfig');
window.callGo('sendMessage', 'Hello from JS!');
```

### Available Go Functions

- `getVersion()` - Returns version string
- `getConfig()` - Returns configuration map
- `sendMessage(message)` - Sends a message to the backend
- `getSystemInfo()` - Returns system information

## Theme

The UI uses OpenFang's color scheme:
- Light mode: #F5F4F2 background, #FF5C00 accent (orange)
- Dark mode: #080706 background, #FF5C00 accent

### Switching Themes

```javascript
// In JavaScript
setTheme('light');  // Light mode
setTheme('dark');   // Dark mode
```

## Next Steps

To integrate this into NemesisBot:

1. Copy `test/desktop-ui` code to `module/desktop/`
2. Update `nemesisbot/main.go` to add `desktop` command
3. Connect to existing AgentLoop and MessageBus
4. Implement real WebSocket communication
5. Add more pages (Agents, Channels, Settings)

## Architecture

```
┌─────────────────────────────────────┐
│   Desktop Window (WebView)         │
├─────────────────────────────────────┤
│                                     │
│  ┌──────────────────────────────┐  │
│  │   Go Backend                │  │
│  │   - HTTP Server             │  │
│  │   - Go ↔ JS Bindings        │  │
│  └──────────────┬───────────────┘  │
│                 │                   │
│                 ▼                   │
│  ┌──────────────────────────────┐  │
│  │   Embedded Static Assets    │  │
│  │   - HTML/CSS/JS             │  │
│  └──────────────────────────────┘  │
└─────────────────────────────────────┘
```

## License

MIT
