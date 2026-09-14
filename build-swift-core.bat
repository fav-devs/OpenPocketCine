@echo off
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set PATH=%PATH%;C:\Users\itztr\AppData\Local\Programs\Swift\Toolchains\6.3.3+Asserts\usr\bin;C:\Users\itztr\AppData\Local\Programs\Swift\Runtimes\6.3.3\usr\bin;C:\VulkanSDK\1.4.357.0\Bin
set SDKROOT=C:\Users\itztr\AppData\Local\Programs\Swift\Platforms\6.3.3\Windows.platform\Developer\SDKs\Windows.sdk
set OPC_CORE_LIB_DIR=C:\Users\itztr\OneDrive\Pictures\git\OpenPocketCine\.build\x86_64-unknown-windows-msvc\debug
set FFMPEG_DIR=C:\Users\itztr\ffmpeg-dev\ffmpeg-master-latest-win64-gpl-shared

echo Building Swift core...
swift build --package-path "C:\Users\itztr\OneDrive\Pictures\git\OpenPocketCine" --product OpenPocketCineDesktop
if errorlevel 1 exit /b 1

echo Forcing clean rebuild of opc-monitor and opc-core-sys...
cd /d "C:\Users\itztr\OneDrive\Pictures\git\OpenPocketCine\Apps\Desktop"
cargo clean -p opc-monitor -p opc-core-sys
if errorlevel 1 exit /b 1

echo Building opc-monitor and the virtual camera source...
cargo build -p opc-monitor -p opc-vcam-win
if errorlevel 1 exit /b 1

echo Staging DLLs next to exe...
set EXE_DIR=C:\Users\itztr\OneDrive\Pictures\git\OpenPocketCine\Apps\Desktop\target\debug
set SWIFT_RT=C:\Users\itztr\AppData\Local\Programs\Swift\Runtimes\6.3.3\usr\bin
set SWIFT_TC=C:\Users\itztr\AppData\Local\Programs\Swift\Toolchains\6.3.3+Asserts\usr\bin
set CORE_DLL=C:\Users\itztr\OneDrive\Pictures\git\OpenPocketCine\.build\x86_64-unknown-windows-msvc\debug\OpenPocketCineDesktop.dll
copy /Y "%CORE_DLL%" "%EXE_DIR%\"
copy /Y "%SWIFT_RT%\*.dll" "%EXE_DIR%\"
copy /Y "%SWIFT_TC%\mimalloc.dll" "%EXE_DIR%\"
copy /Y "%SWIFT_TC%\mimalloc-redirect.dll" "%EXE_DIR%\"
echo Done. Run: Apps\Desktop\target\debug\opc-monitor.exe
echo The camera source opc_vcam_win.dll is beside it; Settings ^> Output ^> Install registers it.
