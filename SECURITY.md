# Security

Ansa is an unreviewed development prototype. Do not use it for secrets or treat it as a sandbox. There is no extension runtime or automatic clipboard capture in this slice.

Do not include queries, clipboard data, credentials, private paths or full memory dumps in public bug reports. No private security contact has been established; until maintainers publish one, do not disclose exploitable details in public issues. Production distribution is blocked on an actual private disclosure channel, threat-model review, dependency audit, signing and recovery tests.

The application must run without elevation. Native OS restrictions must not be bypassed by running it as administrator. App discovery does not execute discovered shortcuts; explicit user invocation can execute the target with the user's normal privileges.
