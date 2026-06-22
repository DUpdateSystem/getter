@getter-cli @autogen
Feature: Installed app autogen
  Scenario: User previews installed app fallback generation without writing files
    Given an initialized getter data directory
    And an installed inventory with Android app "com.example.autogen" labeled "Example Autogen"
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And the output is valid JSON
    And the autogen preview contains candidate "android/com.example.autogen"
    And the local_autogen repository has not been written

  Scenario: User applies installed app autogen and evaluates the generated fallback package
    Given an initialized getter data directory
    And an installed inventory with Android app "com.example.autogen" labeled "Example Autogen"
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And I save the autogen preview to a file
    When I run getter autogen installed apply for that preview with accept-all
    Then the command succeeds
    And the local_autogen repository contains generated package "android/com.example.autogen"
    And the app list contains autogen tracked package "android/com.example.autogen"
    When I run getter repo validate for local_autogen
    Then the output reports a valid repository without network
    When I run getter package eval for package "android/com.example.autogen" from local_autogen
    Then the output contains package "android/com.example.autogen" named "Example Autogen"

  Scenario: Higher-priority repositories suppress installed app autogen candidates
    Given an initialized getter data directory
    And a fixture Lua repository "official" with package "android/com.example.autogen" named "Official Example"
    And an installed inventory with Android app "com.example.autogen" labeled "Example Autogen"
    When I run getter repo add for that repository with priority 0
    Then the command succeeds
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And the autogen preview skips package "android/com.example.autogen" because repository "official" covers it

  Scenario: Cleanup deletes only accepted generated packages missing from installed inventory
    Given an initialized getter data directory
    And an installed inventory with Android app "com.example.old" labeled "Old App"
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And I save the autogen preview to a file
    When I run getter autogen installed apply for that preview with accept-all
    Then the command succeeds
    Given an empty installed inventory
    When I run getter autogen cleanup preview for that inventory
    Then the command succeeds
    And the autogen cleanup preview contains delete candidate "android/com.example.old"
    And I save the autogen preview to a file
    When I run getter autogen cleanup apply for that preview with accept-all
    Then the command succeeds
    And the local_autogen repository does not contain generated package "android/com.example.old"
    And the app list does not contain package "android/com.example.old"

  Scenario: Cleanup rejects tampered previews for non-autogen tracked packages
    Given an initialized getter data directory
    And a syntactically valid legacy export bundle with an Android app
    And a tampered autogen cleanup preview for package "android/org.fdroid.fdroid"
    When I run getter legacy import-room-bundle for that bundle
    Then the command succeeds
    When I run getter autogen cleanup apply for that preview with accept-all
    Then the command fails with an autogen error
    And the app list contains imported package "android/org.fdroid.fdroid"

  Scenario: Applying autogen preserves existing tracked user state
    Given an initialized getter data directory
    And a syntactically valid legacy export bundle with an Android app
    When I run getter legacy import-room-bundle for that bundle
    Then the command succeeds
    Given an installed inventory with Android app "org.fdroid.fdroid" labeled "F-Droid"
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And I save the autogen preview to a file
    When I run getter autogen installed apply for that preview with accept-all
    Then the command succeeds
    And the app list contains imported package "android/org.fdroid.fdroid"

  Scenario: Applying over modified autogen preserves the user-edited file into local
    Given an initialized getter data directory
    And an installed inventory with Android app "com.example.autogen" labeled "Example Autogen"
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And I save the autogen preview to a file
    When I run getter autogen installed apply for that preview with accept-all
    Then the command succeeds
    And I replace generated autogen package "android/com.example.autogen" with user-edited content
    When I run getter autogen installed preview for that inventory
    Then the command succeeds
    And I save the autogen preview to a file
    When I run getter autogen installed apply for that preview with accept-all
    Then the command succeeds
    And local repository contains preserved package "android/com.example.autogen"
