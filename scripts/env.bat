@echo off
rem Shared build environment: MSVC + Ninja + CMake + LLVM for whisper.cpp's CUDA backend.
rem Avoids the Visual Studio generator, which needs CUDA's MSBuild integration (admin to install).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\CMake\bin;C:\Program Files\LLVM\bin;C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;%PATH%"
set "LIBCLANG_PATH=C:\Program Files\LLVM\bin"
set "CMAKE_GENERATOR=Ninja"
rem RTX 5070 = Blackwell sm_120. Override with FS_CUDA_ARCH for other GPUs.
if "%FS_CUDA_ARCH%"=="" set "FS_CUDA_ARCH=120"
set "CMAKE_CUDA_ARCHITECTURES=%FS_CUDA_ARCH%"
