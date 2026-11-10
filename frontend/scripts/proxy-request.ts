import { request as httpRequest } from "node:http";

export function refusedBeforeBodyCompletes({ url, credential, chunked = false, bytes = 256 * 1024 }: {
  url: string;
  credential?: string;
  chunked?: boolean;
  bytes?: number;
}) {
  return new Promise<number | undefined>((resolve, reject) => {
    const req = httpRequest(url, {
      method: "POST",
      headers: {
        ...(chunked ? { "transfer-encoding": "chunked" } : { "content-length": String(20 * 1024 * 1024) }),
        ...(credential ? { authorization: credential } : {}),
      },
    }, (res) => {
      resolve(res.statusCode);
      res.resume();
      req.destroy();
    });
    req.on("error", reject);
    req.setTimeout(3000, () => req.destroy(new Error("frontend waited for the body")));
    // At most 256 KiB is sent. The other 19.75 MiB is withheld, and the request is not ended.
    // A cloning proxy waits for EOF and fails this test; a pre-body auth gate responds now.
    req.write(Buffer.alloc(bytes, 120));
  });
}
