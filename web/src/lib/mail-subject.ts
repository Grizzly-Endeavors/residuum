/** A thread's subject as it is shown: an empty one reads "(no subject)", as mail apps show it. */
export function displaySubject(subject: string): string {
  return subject.trim() === "" ? "(no subject)" : subject;
}
