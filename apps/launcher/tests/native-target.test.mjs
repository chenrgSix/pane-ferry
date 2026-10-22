import test from "node:test";
import assert from "node:assert/strict";
import { endpointKey } from "../src/launcher-types.ts";
test("paired identities isolate devices and survive address changes without sharing SSH cache", () => {
  const a={host:"10.0.1.1",user:"",port:42424,peerId:"device-a"};
  assert.equal(endpointKey(a),endpointKey({...a,host:"10.0.1.2",port:4444}));
  assert.notEqual(endpointKey(a),endpointKey({...a,peerId:"device-b"}));
  assert.notEqual(endpointKey(a),endpointKey({host:a.host,user:a.user,port:a.port}));
});
