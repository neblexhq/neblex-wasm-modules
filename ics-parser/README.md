# ics-parser

Turns iCalendar (`.ics`) data into event records: meeting invites and cancellations attached to emails, calendar feeds and exports. Use it with `email-parser` to turn an invite in a shared inbox into a CRM activity, a booking in another system, or a reminder task.

## Input

One record or an array of records, each with the calendar text:

```json
{ "ics": "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n..." }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the calendar text | `ics` |
| `base64` | The text is base64-encoded, as email attachments often are (use `email-parser` with `include_attachments`) | `false` |
| `keep_source` | Keep the calendar text in the output | `false` |

## Output

```json
{
  "method": "REQUEST",
  "event_count": 2,
  "events": [
    {
      "uid": "abc-123@example.com",
      "summary": "Order 45-1180 review, with Globex",
      "description": "Agenda:\n1. Damaged box\n2. Replacement",
      "location": "https://meet.example.com/abc",
      "start": "2026-11-12T10:00:00", "end": "2026-11-12T10:30:00", "timezone": "America/New_York", "all_day": false,
      "organizer": { "name": "Doe, Jane", "email": "jane.doe@example.com" },
      "attendees": [{ "name": "Sam Lee", "email": "sam@globex.com", "role": "REQ-PARTICIPANT", "status": "ACCEPTED", "rsvp": true }]
    },
    { "uid": "holiday-1", "summary": "Office closed", "start": "2026-11-26", "all_day": true, "timezone": null, "attendees": [] }
  ],
  "ok": true
}
```

- `method` says what the invite is: `REQUEST` (new or updated), `CANCEL`, `REPLY` and so on. Use `uid` with `sequence` to update the right record when an invite changes.
- Times are ISO 8601. A time ending in `Z` is UTC (`timezone` is `"UTC"`); otherwise the time is local to `timezone`, as written in the invite. All-day events have a plain date.
- Also returned when present: `status`, `sequence`, `recurrence` (the RRULE as written, such as `FREQ=WEEKLY;COUNT=3`), `duration`, `url`, `created`, `last_modified`.
- Text escapes (`\n`, `\,`) are decoded and folded lines joined. Alarms and time zone definitions are skipped.

Recurring events are returned once, with their rule; the module doesn't expand them into individual dates.

## Build and test

```sh
../build.sh ics-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input (an invite and an all-day event) for trying the module in a flow or with `../test/call.py`.
