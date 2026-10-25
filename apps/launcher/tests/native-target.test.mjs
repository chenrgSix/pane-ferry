import test from "node:test";
import assert from "node:assert/strict";
import { endpointKey, validPeerId } from "../src/launcher-types.ts";
import { retireLegacyPreferences } from "../src/connection-preferences.ts";
const a={peerId:"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"};
const b={peerId:"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"};
test("paired UUIDs isolate devices regardless of name or address", () => {
  assert.equal(endpointKey(a),endpointKey({...a,address:"10.0.1.2:4444",name:"renamed"}));
  assert.notEqual(endpointKey(a),endpointKey(b));
  for (const id of ["", "host", "../route", a.peerId.toUpperCase()]) assert.equal(validPeerId(id),false);
  assert.equal(validPeerId(a.peerId),true);
});
test("migration preserves both peers' caches and ambiguous launches, removes legacy profiles", () => {
  const old=JSON.stringify({host:"ubuntu",user:"alice",port:22});
  const catalogs=[[endpointKey(a),{apps:[]}],[endpointKey(b),{apps:[]}],[old,{apps:[]}]];
  const pending=[[endpointKey(a)+":app:native:auto","request-a"],[endpointKey(b)+":app:native:auto","request-b"],[old+":app:native:auto","legacy"]];
  const data=new Map([
    ["lan-remoteapp.connection",JSON.stringify({endpoint:{...a,host:"old",user:"old",port:22},renderScale:"2",applicationScene:true})],
    ["lan-remoteapp.devices","legacy"],
    ["lan-remoteapp.app-catalogs.v1",JSON.stringify(catalogs)],
    ["lan-remoteapp.pending-launches",JSON.stringify(pending)],
  ]);
  const storage={getItem:k=>data.get(k)??null,setItem:(k,v)=>data.set(k,v),removeItem:k=>data.delete(k)};
  retireLegacyPreferences(storage); retireLegacyPreferences(storage);
  assert.deepEqual(JSON.parse(data.get("lan-remoteapp.connection")),{endpoint:a,applicationScene:true});
  assert.deepEqual(JSON.parse(data.get("lan-remoteapp.app-catalogs.v1")),catalogs.slice(0,2));
  assert.deepEqual(JSON.parse(data.get("lan-remoteapp.pending-launches")),pending.slice(0,2));
  assert.equal(data.has("lan-remoteapp.devices"),false);
});
