// Calls into the provider through its PLT, which is what the import hook
// rewrites. Deliberately compiled without -fno-plt: the point of the test is the
// path a real engine takes when it calls eglSwapBuffers.

extern "C" int pl_provider_value();
extern "C" int pl_provider_add(int left, int right);

extern "C" int pl_consumer_call() { return pl_provider_value(); }

extern "C" int pl_consumer_sum(int left, int right) { return pl_provider_add(left, right); }
