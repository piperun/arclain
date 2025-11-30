@echo off
setlocal enabledelayedexpansion

REM Get the absolute path to the workspace root
set SCRIPT_DIR=%~dp0
set WORKSPACE_ROOT=%SCRIPT_DIR%..
pushd "%WORKSPACE_ROOT%"
set WORKSPACE_ROOT=%CD%
popd

set PLUGINS_DIR=%WORKSPACE_ROOT%\plugins
set TARGET=wasm32-wasip2

echo Building WASM plugins...
echo Workspace: %WORKSPACE_ROOT%
echo.

REM Check if target is installed
rustup target list --installed | findstr /C:"%TARGET%" >nul
if errorlevel 1 (
    echo Target %TARGET% not installed. Installing...
    rustup target add %TARGET%
)

REM Build each plugin
set BUILD_FAILED=0
for /d %%p in (%PLUGINS_DIR%\*) do (
    if exist "%%p\Cargo.toml" (
        set "plugin_name=%%~nxp"
        echo.
        echo Building !plugin_name!...
        
        pushd "%%p"
        
        REM Build for WASM in plugin's own target directory
        cargo build --target %TARGET% --release --target-dir .
        
        if errorlevel 1 (
            echo ERROR: Failed to build !plugin_name!
            set BUILD_FAILED=1
            popd
            goto :continue
        )
        
        REM Determine the WASM file name (replace hyphens with underscores)
        set "wasm_name=!plugin_name:-=_!"
        
        REM WASM file is in plugin's local target directory
        set "wasm_src=%TARGET%\release\!wasm_name!.wasm"
        set "wasm_dest=!plugin_name!.wasm"
        
        REM Componentize the WASM module if needed
        if exist "!wasm_src!" (
            if "%TARGET%"=="wasm32-wasip2" (
                echo Target is wasm32-wasip2, skipping componentization...
                copy /Y "!wasm_src!" "!wasm_dest!" >nul
            ) else (
                echo Componentizing !plugin_name!...
                wasm-tools component new "!wasm_src!" -o "!wasm_dest!"
                
                if errorlevel 1 (
                    echo ERROR: Failed to componentize !plugin_name!
                    set BUILD_FAILED=1
                    popd
                    goto :continue
                )
            )
            
            for %%A in ("!wasm_dest!") do set "file_size=%%~zA"
            echo Built Component: !wasm_dest! (!file_size! bytes)
        ) else (
            echo WARNING: WASM file not found
            echo   Expected: !wasm_src!
            echo   Full path: %%p\!wasm_src!
        )
        
        :continue
        popd
    )
)

echo.
echo All plugins processed

if %BUILD_FAILED%==1 (
    echo.
    echo WARNING: Some plugins failed to build. Check errors above.
    goto :end
)

echo.
echo Plugin files created:
set FOUND_PLUGINS=0
for /d %%p in (%PLUGINS_DIR%\*) do (
    set "plugin_name=%%~nxp"
    set "wasm_file=%%p\!plugin_name!.wasm"
    if exist "!wasm_file!" (
        for %%A in ("!wasm_file!") do set "file_size=%%~zA"
        echo   - !wasm_file! (!file_size! bytes)
        set /a FOUND_PLUGINS+=1
    )
)

if %FOUND_PLUGINS%==0 (
    echo   WARNING: No plugin WASM files found!
    echo   Check the build output above for errors.
)

:end
endlocal