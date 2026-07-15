# complete code
import os
import json
import fs
from fs import open_file
from fs import error as fs_error

class Accounts:
    def __init__(self):
        self.accounts_path = os.path.join(os.path.expanduser('~'), '.codex-switcher', 'accounts.json')

    def load_accounts(self) -> list:
        """
        Load accounts from the accounts.json file.

        Returns:
            A list of dictionaries representing the accounts.
        """
        try:
            with open_file(self.accounts_path, 'r') as f:
                return json.load(f)
        except fs_error.FileNotFoundError:
            # Handle the case where the file does not exist
            return []
        except json.JSONDecodeError as e:
            # Handle the case where the file is not a valid JSON
            print(f"Error parsing accounts file: {e}")
            return []
        except Exception as e:
            # Handle any other exceptions
            print(f"Error loading accounts: {e}")
            return []

    def save_accounts(self, accounts: list) -> None:
        """
        Save accounts to the accounts.json file.

        Args:
            accounts: A list of dictionaries representing the accounts.
        """
        try:
            with open_file(self.accounts_path, 'w') as f:
                json.dump(accounts, f)
        except Exception as e:
            # Handle any exceptions
            print(f"Error saving accounts: {e}")