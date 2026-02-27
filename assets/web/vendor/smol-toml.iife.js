(()=>{function Z(e,n){let i=e.slice(0,n).split(/\r\n|\n|\r/g);return[i.length,i.pop().length+1]}function j(e,n,i){let t=e.split(/\r\n|\n|\r/g),o="",l=(Math.log10(n+1)|0)+1;for(let r=n-1;r<=n+1;r++){let f=t[r-1];f&&(o+=r.toString().padEnd(l," "),o+=":  ",o+=f,o+=`
`,r===n&&(o+=" ".repeat(l+i+2),o+=`^
`))}return o}var c=class extends Error{line;column;codeblock;constructor(n,i){let[t,o]=Z(i.toml,i.ptr),l=j(i.toml,t,o);super(`Invalid TOML document: ${n}

${l}`,i),this.line=t,this.column=o,this.codeblock=l}};function z(e,n){let i=0;for(;e[n-++i]==="\\";);return--i&&i%2}function b(e,n=0,i=e.length){let t=e.indexOf(`
`,n);return e[t-1]==="\r"&&t--,t<=i?t:-1}function h(e,n){for(let i=n;i<e.length;i++){let t=e[i];if(t===`
`)return i;if(t==="\r"&&e[i+1]===`
`)return i+1;if(t<" "&&t!=="	"||t==="\x7F")throw new c("control characters are not allowed in comments",{toml:e,ptr:n})}return e.length}function s(e,n,i,t){let o;for(;(o=e[n])===" "||o==="	"||!i&&(o===`
`||o==="\r"&&e[n+1]===`
`);)n++;return t||o!=="#"?n:s(e,h(e,n),i)}function D(e,n,i,t,o=!1){if(!t)return n=b(e,n),n<0?e.length:n;for(let l=n;l<e.length;l++){let r=e[l];if(r==="#")l=b(e,l);else{if(r===i)return l+1;if(r===t||o&&(r===`
`||r==="\r"&&e[l+1]===`
`))return l}}throw new c("cannot find end of structure",{toml:e,ptr:n})}function E(e,n){let i=e[n],t=i===e[n+1]&&e[n+1]===e[n+2]?e.slice(n,n+3):i;n+=t.length-1;do n=e.indexOf(t,++n);while(n>-1&&i!=="'"&&z(e,n));return n>-1&&(n+=t.length,t.length>1&&(e[n]===i&&n++,e[n]===i&&n++)),n}var M=/^(\d{4}-\d{2}-\d{2})?[T ]?(?:(\d{2}):\d{2}(?::\d{2}(?:\.\d+)?)?)?(Z|[-+]\d{2}:\d{2})?$/i,g=class e extends Date{#n=!1;#t=!1;#e=null;constructor(n){let i=!0,t=!0,o="Z";if(typeof n=="string"){let l=n.match(M);l?(l[1]||(i=!1,n=`0000-01-01T${n}`),t=!!l[2],t&&n[10]===" "&&(n=n.replace(" ","T")),l[2]&&+l[2]>23?n="":(o=l[3]||null,n=n.toUpperCase(),!o&&t&&(n+="Z"))):n=""}super(n),isNaN(this.getTime())||(this.#n=i,this.#t=t,this.#e=o)}isDateTime(){return this.#n&&this.#t}isLocal(){return!this.#n||!this.#t||!this.#e}isDate(){return this.#n&&!this.#t}isTime(){return this.#t&&!this.#n}isValid(){return this.#n||this.#t}toISOString(){let n=super.toISOString();if(this.isDate())return n.slice(0,10);if(this.isTime())return n.slice(11,23);if(this.#e===null)return n.slice(0,-1);if(this.#e==="Z")return n;let i=+this.#e.slice(1,3)*60+ +this.#e.slice(4,6);return i=this.#e[0]==="-"?i:-i,new Date(this.getTime()-i*6e4).toISOString().slice(0,-1)+this.#e}static wrapAsOffsetDateTime(n,i="Z"){let t=new e(n);return t.#e=i,t}static wrapAsLocalDateTime(n){let i=new e(n);return i.#e=null,i}static wrapAsLocalDate(n){let i=new e(n);return i.#t=!1,i.#e=null,i}static wrapAsLocalTime(n){let i=new e(n);return i.#n=!1,i.#e=null,i}};var v=/^((0x[0-9a-fA-F](_?[0-9a-fA-F])*)|(([+-]|0[ob])?\d(_?\d)*))$/,G=/^[+-]?\d(_?\d)*(\.\d(_?\d)*)?([eE][+-]?\d(_?\d)*)?$/,U=/^[+-]?0[0-9_]/,K=/^[0-9a-f]{2,8}$/i,N={b:"\b",t:"	",n:`
`,f:"\f",r:"\r",e:"\x1B",'"':'"',"\\":"\\"};function T(e,n=0,i=e.length){let t=e[n]==="'",o=e[n++]===e[n]&&e[n]===e[n+1];o&&(i-=2,e[n+=2]==="\r"&&n++,e[n]===`
`&&n++);let l=0,r,f="",u=n;for(;n<i-1;){let a=e[n++];if(a===`
`||a==="\r"&&e[n]===`
`){if(!o)throw new c("newlines are not allowed in strings",{toml:e,ptr:n-1})}else if(a<" "&&a!=="	"||a==="\x7F")throw new c("control characters are not allowed in strings",{toml:e,ptr:n-1});if(r){if(r=!1,a==="x"||a==="u"||a==="U"){let d=e.slice(n,n+=a==="x"?2:a==="u"?4:8);if(!K.test(d))throw new c("invalid unicode escape",{toml:e,ptr:l});try{f+=String.fromCodePoint(parseInt(d,16))}catch{throw new c("invalid unicode escape",{toml:e,ptr:l})}}else if(o&&(a===`
`||a===" "||a==="	"||a==="\r")){if(n=s(e,n-1,!0),e[n]!==`
`&&e[n]!=="\r")throw new c("invalid escape: only line-ending whitespace may be escaped",{toml:e,ptr:l});n=s(e,n)}else if(a in N)f+=N[a];else throw new c("unrecognized escape sequence",{toml:e,ptr:l});u=n}else!t&&a==="\\"&&(l=n-1,r=!0,f+=e.slice(u,l))}return f+e.slice(u,i-1)}function A(e,n,i,t){if(e==="true")return!0;if(e==="false")return!1;if(e==="-inf")return-1/0;if(e==="inf"||e==="+inf")return 1/0;if(e==="nan"||e==="+nan"||e==="-nan")return NaN;if(e==="-0")return t?0n:0;let o=v.test(e);if(o||G.test(e)){if(U.test(e))throw new c("leading zeroes are not allowed",{toml:n,ptr:i});e=e.replace(/_/g,"");let r=+e;if(isNaN(r))throw new c("invalid number",{toml:n,ptr:i});if(o){if((o=!Number.isSafeInteger(r))&&!t)throw new c("integer value cannot be represented losslessly",{toml:n,ptr:i});(o||t===!0)&&(r=BigInt(e))}return r}let l=new g(e);if(!l.isValid())throw new c("invalid value",{toml:n,ptr:i});return l}function X(e,n,i){let t=e.slice(n,i),o=t.indexOf("#");return o>-1&&(h(e,o),t=t.slice(0,o)),[t.trimEnd(),o]}function p(e,n,i,t,o){if(t===0)throw new c("document contains excessively nested structures. aborting.",{toml:e,ptr:n});let l=e[n];if(l==="["||l==="{"){let[u,a]=l==="["?C(e,n,t,o):V(e,n,t,o);if(i){if(a=s(e,a),e[a]===",")a++;else if(e[a]!==i)throw new c("expected comma or end of structure",{toml:e,ptr:a})}return[u,a]}let r;if(l==='"'||l==="'"){r=E(e,n);let u=T(e,n,r);if(i){if(r=s(e,r),e[r]&&e[r]!==","&&e[r]!==i&&e[r]!==`
`&&e[r]!=="\r")throw new c("unexpected character encountered",{toml:e,ptr:r});r+=+(e[r]===",")}return[u,r]}r=D(e,n,",",i);let f=X(e,n,r-+(e[r-1]===","));if(!f[0])throw new c("incomplete key-value declaration: no value specified",{toml:e,ptr:n});return i&&f[1]>-1&&(r=s(e,n+f[1]),r+=+(e[r]===",")),[A(f[0],e,n,o),r]}var Y=/^[a-zA-Z0-9-_]+[ \t]*$/;function O(e,n,i="="){let t=n-1,o=[],l=e.indexOf(i,n);if(l<0)throw new c("incomplete key-value: cannot find end of key",{toml:e,ptr:n});do{let r=e[n=++t];if(r!==" "&&r!=="	")if(r==='"'||r==="'"){if(r===e[n+1]&&r===e[n+2])throw new c("multiline strings are not allowed in keys",{toml:e,ptr:n});let f=E(e,n);if(f<0)throw new c("unfinished string encountered",{toml:e,ptr:n});t=e.indexOf(".",f);let u=e.slice(f,t<0||t>l?l:t),a=b(u);if(a>-1)throw new c("newlines are not allowed in keys",{toml:e,ptr:n+t+a});if(u.trimStart())throw new c("found extra tokens after the string part",{toml:e,ptr:f});if(l<f&&(l=e.indexOf(i,f),l<0))throw new c("incomplete key-value: cannot find end of key",{toml:e,ptr:n});o.push(T(e,n,f))}else{t=e.indexOf(".",n);let f=e.slice(n,t<0||t>l?l:t);if(!Y.test(f))throw new c("only letter, numbers, dashes and underscores are allowed in keys",{toml:e,ptr:n});o.push(f.trimEnd())}}while(t+1&&t<l);return[o,s(e,l+1,!0,!0)]}function V(e,n,i,t){let o={},l=new Set,r;for(n++;(r=e[n++])!=="}"&&r;){if(r===",")throw new c("expected value, found comma",{toml:e,ptr:n-1});if(r==="#")n=h(e,n);else if(r!==" "&&r!=="	"&&r!==`
`&&r!=="\r"){let f,u=o,a=!1,[d,m]=O(e,n-1);for(let x=0;x<d.length;x++){if(x&&(u=a?u[f]:u[f]={}),f=d[x],(a=Object.hasOwn(u,f))&&(typeof u[f]!="object"||l.has(u[f])))throw new c("trying to redefine an already defined value",{toml:e,ptr:n});!a&&f==="__proto__"&&Object.defineProperty(u,f,{enumerable:!0,configurable:!0,writable:!0})}if(a)throw new c("trying to redefine an already defined value",{toml:e,ptr:n});let[w,R]=p(e,m,"}",i-1,t);l.add(w),u[f]=w,n=R}}if(!r)throw new c("unfinished table encountered",{toml:e,ptr:n});return[o,n]}function C(e,n,i,t){let o=[],l;for(n++;(l=e[n++])!=="]"&&l;){if(l===",")throw new c("expected value, found comma",{toml:e,ptr:n-1});if(l==="#")n=h(e,n);else if(l!==" "&&l!=="	"&&l!==`
`&&l!=="\r"){let r=p(e,n-1,"]",i-1,t);o.push(r[0]),n=r[1]}}if(!l)throw new c("unfinished array encountered",{toml:e,ptr:n});return[o,n]}function L(e,n,i,t){let o=n,l=i,r,f=!1,u;for(let a=0;a<e.length;a++){if(a){if(o=f?o[r]:o[r]={},l=(u=l[r]).c,t===0&&(u.t===1||u.t===2))return null;if(u.t===2){let d=o.length-1;o=o[d],l=l[d].c}}if(r=e[a],(f=Object.hasOwn(o,r))&&l[r]?.t===0&&l[r]?.d)return null;f||(r==="__proto__"&&(Object.defineProperty(o,r,{enumerable:!0,configurable:!0,writable:!0}),Object.defineProperty(l,r,{enumerable:!0,configurable:!0,writable:!0})),l[r]={t:a<e.length-1&&t===2?3:t,d:!1,i:0,c:{}})}if(u=l[r],u.t!==t&&!(t===1&&u.t===3)||(t===2&&(u.d||(u.d=!0,o[r]=[]),o[r].push(o={}),u.c[u.i++]=u={t:1,d:!1,i:0,c:{}}),u.d))return null;if(u.d=!0,t===1)o=f?o[r]:o[r]={};else if(t===0&&f)return null;return[r,o,u.c]}function S(e,{maxDepth:n=1e3,integersAsBigInt:i}={}){let t={},o={},l=t,r=o;for(let f=s(e,0);f<e.length;){if(e[f]==="["){let u=e[++f]==="[",a=O(e,f+=+u,"]");if(u){if(e[a[1]-1]!=="]")throw new c("expected end of table declaration",{toml:e,ptr:a[1]-1});a[1]++}let d=L(a[0],t,o,u?2:1);if(!d)throw new c("trying to redefine an already defined table or value",{toml:e,ptr:f});r=d[2],l=d[1],f=a[1]}else{let u=O(e,f),a=L(u[0],l,r,0);if(!a)throw new c("trying to redefine an already defined table or value",{toml:e,ptr:f});let d=p(e,u[1],void 0,n,i);a[1][a[0]]=d[0],f=d[1]}if(f=s(e,f,!0),e[f]&&e[f]!==`
`&&e[f]!=="\r")throw new c("each key-value declaration must be followed by an end-of-line",{toml:e,ptr:f});f=s(e,f)}return t}var P=/^[a-z0-9-_]+$/i;function y(e){let n=typeof e;if(n==="object"){if(Array.isArray(e))return"array";if(e instanceof Date)return"date"}return n}function q(e){for(let n=0;n<e.length;n++)if(y(e[n])!=="object")return!1;return e.length!=0}function _(e){return JSON.stringify(e).replace(/\x7f/g,"\\u007f")}function $(e,n,i,t){if(i===0)throw new Error("Could not stringify the object: maximum object depth exceeded");if(n==="number")return isNaN(e)?"nan":e===1/0?"inf":e===-1/0?"-inf":t&&Number.isInteger(e)?e.toFixed(1):e.toString();if(n==="bigint"||n==="boolean")return e.toString();if(n==="string")return _(e);if(n==="date"){if(isNaN(e.getTime()))throw new TypeError("cannot serialize invalid date");return e.toISOString()}if(n==="object")return F(e,i,t);if(n==="array")return J(e,i,t)}function F(e,n,i){let t=Object.keys(e);if(t.length===0)return"{}";let o="{ ";for(let l=0;l<t.length;l++){let r=t[l];l&&(o+=", "),o+=P.test(r)?r:_(r),o+=" = ",o+=$(e[r],y(e[r]),n-1,i)}return o+" }"}function J(e,n,i){if(e.length===0)return"[]";let t="[ ";for(let o=0;o<e.length;o++){if(o&&(t+=", "),e[o]===null||e[o]===void 0)throw new TypeError("arrays cannot contain null or undefined values");t+=$(e[o],y(e[o]),n-1,i)}return t+" ]"}function H(e,n,i,t){if(i===0)throw new Error("Could not stringify the object: maximum object depth exceeded");let o="";for(let l=0;l<e.length;l++)o+=`${o&&`
`}[[${n}]]
`,o+=k(0,e[l],n,i,t);return o}function k(e,n,i,t,o){if(t===0)throw new Error("Could not stringify the object: maximum object depth exceeded");let l="",r="",f=Object.keys(n);for(let u=0;u<f.length;u++){let a=f[u];if(n[a]!==null&&n[a]!==void 0){let d=y(n[a]);if(d==="symbol"||d==="function")throw new TypeError(`cannot serialize values of type '${d}'`);let m=P.test(a)?a:_(a);if(d==="array"&&q(n[a]))r+=(r&&`
`)+H(n[a],i?`${i}.${m}`:m,t-1,o);else if(d==="object"){let w=i?`${i}.${m}`:m;r+=(r&&`
`)+k(w,n[a],w,t-1,o)}else l+=m,l+=" = ",l+=$(n[a],d,t,o),l+=`
`}}return e&&(l||!r)&&(l=l?`[${e}]
${l}`:`[${e}]`),l&&r?`${l}
${r}`:l||r}function I(e,{maxDepth:n=1e3,numbersAsFloat:i=!1}={}){if(y(e)!=="object")throw new TypeError("stringify can only be called with an object");let t=k(0,e,"",n,i);return t[t.length-1]!==`
`?t+`
`:t}window.TOML={parse:S,stringify:I};})();
/*! Bundled license information:

smol-toml/dist/error.js:
smol-toml/dist/util.js:
smol-toml/dist/date.js:
smol-toml/dist/primitive.js:
smol-toml/dist/extract.js:
smol-toml/dist/struct.js:
smol-toml/dist/parse.js:
smol-toml/dist/stringify.js:
smol-toml/dist/index.js:
  (*!
   * Copyright (c) Squirrel Chat et al., All rights reserved.
   * SPDX-License-Identifier: BSD-3-Clause
   *
   * Redistribution and use in source and binary forms, with or without
   * modification, are permitted provided that the following conditions are met:
   *
   * 1. Redistributions of source code must retain the above copyright notice, this
   *    list of conditions and the following disclaimer.
   * 2. Redistributions in binary form must reproduce the above copyright notice,
   *    this list of conditions and the following disclaimer in the
   *    documentation and/or other materials provided with the distribution.
   * 3. Neither the name of the copyright holder nor the names of its contributors
   *    may be used to endorse or promote products derived from this software without
   *    specific prior written permission.
   *
   * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
   * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
   * WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
   * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
   * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
   * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
   * SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
   * CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
   * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
   * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
   *)
*/
