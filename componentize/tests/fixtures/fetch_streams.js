// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// The `stream<u8>` and `fetch` guest module. A lifted `stream<u8>` is a
// `ReadableStream` of bytes, which `fetch` takes as a request body, and a
// response's `body` is a `ReadableStream` of bytes, which lowers to a
// `stream<u8>`.

async function postStream(body) {
    const response = await fetch("http://example.com/echo", {
        method: "POST",
        body,
        // @ts-expect-error TypeScript's `RequestInit` has no `duplex` member.
        duplex: "half",
    })
    return await response.text()
}

async function getBody() {
    const response = await fetch("http://example.com/data")
    return response.body
}

async function postAndStream(body) {
    const response = await fetch("http://example.com/echo", {
        method: "POST",
        body,
        // @ts-expect-error TypeScript's `RequestInit` has no `duplex` member.
        duplex: "half",
    })
    return response.body
}

/** @type {Response[]} */
let held = []
/** @type {string[]} */
let readMessages = []

async function holdResponses() {
    held = [await fetch("http://example.com/data"), await fetch("http://example.com/data")]
}

/** @param {Promise<unknown>} read */
function recordRead(read) {
    read.then(
        () => { readMessages.push("the read succeeded in a sync export") },
        (e) => { readMessages.push(e instanceof TypeError ? e.message : String(e)) },
    )
}

function syncReadMessages() {
    const recorded = readMessages
    readMessages = []
    for (const [i, response] of held.entries()) {
        recordRead(i == 0 ? response.text() : /** @type {ReadableStream} */ (response.body).getReader().read())
    }
    held = []
    return recorded
}

export const fetching = {
    postStream,
    getBody,
    postAndStream,
    holdResponses,
    syncReadMessages,
}
