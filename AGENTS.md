# Code Style Guidelines

Apply these rules when writing or editing source files.

## Comments

- Keep at most one short sentence at the top of a file describing what it is. Remove long narration comments.
- Every function gets a comment of one concise sentence max. State the contract or intent — never narrate the implementation.
- Only add inline comments for legitimate non-obvious edge cases.

## Comment content

- Keep specifics out of comments: env var names, example values, paths, platform behavior. Anything already visible in the code below the comment doesn't need restating.
- Details owned by other code (e.g. where a name is resolved into a path) belong there, not here. State the contract in general terms; the code is the source of truth for specifics.

## Organization

- Group functions, classes, and other definitions by role or domain.
- Logic may be broken up or consolidated to facilitate readability, as long as behavior is preserved.

## Code

- Write expressive, descriptive code that prioritizes readability over being concise or clever.
