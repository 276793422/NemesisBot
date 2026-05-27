// C++ shim: catch exceptions from sherpa-onnx that would otherwise
// cross the Rust FFI boundary and abort the process.
//
// On Windows, sherpa-onnx is compiled as C++ and may throw exceptions
// for unexpected input (e.g. unknown tokens). Rust cannot catch C++
// exceptions, so we wrap the dangerous calls here with try/catch.

extern "C" {

// Wrap SherpaOnnxOfflineTtsGenerate with try/catch.
// Returns 0 on success (output in *out), -1 on C++ exception.
int safe_tts_generate(
    const void* (*generate_fn)(const void*, const char*, int, float),
    const void* tts,
    const char* text,
    int sid,
    float speed,
    const void** out)
{
    try {
        *out = generate_fn(tts, text, sid, speed);
        return 0;
    } catch (...) {
        *out = nullptr;
        return -1;
    }
}

// Wrap SherpaOnnxCreateOfflineTts with try/catch.
int safe_tts_create(
    const void* (*create_fn)(const void*),
    const void* config,
    const void** out)
{
    try {
        *out = create_fn(config);
        return 0;
    } catch (...) {
        *out = nullptr;
        return -1;
    }
}

} // extern "C"
