// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#include "host_api.h"

namespace host_api {

void handle_api_error(JSContext *cx, uint8_t err, int line, const char *func) {
  JS_ReportErrorUTF8(cx, "%s: An error occurred while using the host API.\n", func);
}

} // namespace host_api
