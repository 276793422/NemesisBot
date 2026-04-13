@echo off
setlocal

set GCC_PATH=C:\msys64\mingw64\bin
set "PATH=%GCC_PATH%;%PATH%"

if "%~1"=="" goto usage

if /i "%~1"=="cli" (
    echo Building CLI client...
    go build -o wsclient-cli.exe ./src/
    if errorlevel 1 (
        echo [ERROR] CLI build failed
        exit /b 1
    )
    echo [OK] wsclient-cli.exe

) else if /i "%~1"=="bridge" (
    echo Building TCP bridge...
    set CGO_ENABLED=0
    go build -o wsclient-bridge.exe .
    if errorlevel 1 (
        echo [ERROR] Bridge build failed
        exit /b 1
    )
    echo [OK] wsclient-bridge.exe

) else if /i "%~1"=="dll" (
    echo Building DLL...
    set CGO_ENABLED=1
    go build -buildmode=c-shared -o wsclient.dll .
    if errorlevel 1 (
        echo [ERROR] DLL build failed
        exit /b 1
    )
    echo [OK] wsclient.dll + wsclient.h

) else if /i "%~1"=="all" (
    echo Building all targets...
    echo.

    echo [1/3] CLI client...
    go build -o wsclient-cli.exe ./src/
    if errorlevel 1 (
        echo [ERROR] CLI build failed
        exit /b 1
    )
    echo [OK] wsclient-cli.exe
    echo.

    echo [2/3] TCP bridge...
    set CGO_ENABLED=0
    go build -o wsclient-bridge.exe .
    if errorlevel 1 (
        echo [ERROR] Bridge build failed
        exit /b 1
    )
    echo [OK] wsclient-bridge.exe
    echo.

    echo [3/3] DLL...
    set CGO_ENABLED=1
    go build -buildmode=c-shared -o wsclient.dll .
    if errorlevel 1 (
        echo [ERROR] DLL build failed
        exit /b 1
    )
    echo [OK] wsclient.dll + wsclient.h
    echo.
    echo All targets built successfully.

) else if /i "%~1"=="clean" (
    echo Cleaning build artifacts...
    del /q wsclient-cli.exe 2>nul
    del /q wsclient-bridge.exe 2>nul
    del /q wsclient.exe 2>nul
    del /q wsclient.dll 2>nul
    del /q wsclient.h 2>nul
    del /q wsclient.lib 2>nul
    echo [OK] Cleaned.

) else (
    goto usage
)

exit /b 0

:usage
echo Usage: build.bat ^<target^>
echo.
echo Targets:
echo   cli     Build CLI interactive client  (wsclient-cli.exe)
echo   bridge  Build TCP bridge server       (wsclient-bridge.exe)
echo   dll     Build shared library          (wsclient.dll + wsclient.h)
echo   all     Build all three targets
echo   clean   Remove build artifacts
echo.
echo Note: DLL build requires GCC (MSYS2 MinGW64) in %GCC_PATH%
exit /b 1
