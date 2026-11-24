export function displaySubject(subject: string): string {
  return subject.trim() === "" ? "(no subject)" : subject;
}
